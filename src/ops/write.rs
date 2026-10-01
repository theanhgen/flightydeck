//! add / follow / remove through Flighty's API (never the local DB).

use std::sync::LazyLock;

use chrono::NaiveDate;
use regex::Regex;
use rusqlite::{Connection, OptionalExtension};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::Client;
use crate::ctx::{Class, Ctx};
use crate::error::{Error, Result};
use crate::render::Render;
use crate::{creds, db, owner, proto, time};

#[derive(Debug, Clone, Deserialize, JsonSchema, clap::Args)]
pub struct AddArgs {
    /// Flight code, e.g. "VN333".
    pub flight: String,
    /// Departure date YYYY-MM-DD (local to the departure airport).
    pub date: String,
    /// Add even if the flight already looks tracked locally.
    #[arg(long)]
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Deserialize, JsonSchema, clap::Args)]
pub struct RemoveArgs {
    /// Flighty flight UUID (from `flightydeck get`).
    pub id: String,
    /// Confirm the removal (required when not on a TTY / always via MCP).
    #[arg(long)]
    #[serde(default)]
    pub yes: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct WriteResult {
    /// "added" | "followed" | "already_tracked" | "removed"
    pub outcome: String,
    pub flight_code: Option<String>,
    pub date: Option<String>,
    pub flight_id: Option<String>,
    pub airline: Option<String>,
    pub message: String,
}

impl Render for WriteResult {
    fn table(&self) -> String {
        self.message.clone()
    }
}

static CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Z]{2}|\d[A-Z]|[A-Z]\d)(\d+)$").unwrap());
static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap());

/// A validated add/follow request.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Wanted {
    iata: String,
    number: String,
    date: String,
    force: bool,
}

impl Wanted {
    fn code(&self) -> String {
        format!("{}{}", self.iata, self.number)
    }
}

fn validate(a: &AddArgs) -> Result<Wanted> {
    let code: String = a
        .flight
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_uppercase();
    let caps = CODE.captures(&code).ok_or_else(|| {
        Error::BadInput(format!(
            "invalid flight code {:?}: expected airline code + number, like \"QR111\" or \"VN 333\"",
            a.flight
        ))
    })?;
    let date = a.date.trim();
    if !DATE.is_match(date) || NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
        return Err(Error::BadInput(format!(
            "invalid date {:?}: expected a real calendar date as YYYY-MM-DD",
            a.date
        )));
    }
    Ok(Wanted {
        iata: caps[1].to_string(),
        number: caps[2].to_string(),
        date: date.to_string(),
        force: a.force,
    })
}

/// Validation, then policy: both before touching the DB, credentials or network.
fn check(ctx: &Ctx, a: &AddArgs) -> Result<Wanted> {
    let w = validate(a)?;
    ctx.guard(Class::Write)?;
    Ok(w)
}

fn owner_id(ctx: &Ctx) -> Result<String> {
    let conn = db::open(ctx)?;
    Ok(owner::resolve(ctx, &conn)?.user_id)
}

/// Add a flight you're on (passenger).
pub fn add(ctx: &Ctx, a: &AddArgs) -> Result<WriteResult> {
    // Validate and check policy before owner resolution, which may read the token.
    check(ctx, a)?;
    add_for(ctx, &owner_id(ctx)?, a, &Client::new(ctx)?)
}

/// Follow a flight you're not on.
pub fn follow(ctx: &Ctx, a: &AddArgs) -> Result<WriteResult> {
    // Validate and check policy before owner resolution, which may read the token.
    check(ctx, a)?;
    follow_for(ctx, &owner_id(ctx)?, a, &Client::new(ctx)?)
}

/// `add` with an explicit owner and client.
pub(crate) fn add_for(
    ctx: &Ctx,
    owner_id: &str,
    a: &AddArgs,
    client: &Client,
) -> Result<WriteResult> {
    track(ctx, owner_id, &check(ctx, a)?, true, client)
}

/// `follow` with an explicit owner and client.
pub(crate) fn follow_for(
    ctx: &Ctx,
    owner_id: &str,
    a: &AddArgs,
    client: &Client,
) -> Result<WriteResult> {
    track(ctx, owner_id, &check(ctx, a)?, false, client)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Airline {
    id: String,
    name: String,
}

/// Every live local airline with `iata`: those in the owner's flights first, then by relevance.
/// IATA codes aren't unique, so the caller tries them all.
fn airline_candidates(conn: &Connection, owner_id: &str, iata: &str) -> Result<Vec<Airline>> {
    let mut stmt = conn.prepare(
        "SELECT al.id, al.name FROM Airline al
         WHERE al.iata = ?1 AND al.deleted IS NULL
         ORDER BY (EXISTS (SELECT 1 FROM UserFlight uf JOIN Flight f ON f.id = uf.flightId
                           WHERE uf.userId = ?2 AND uf.deleted IS NULL AND f.airlineId = al.id)
                   OR EXISTS (SELECT 1 FROM UserManualFlight um JOIN ManualFlight m ON m.id = um.flightId
                              WHERE um.userId = ?2 AND um.deleted IS NULL AND m.airlineId = al.id)) DESC,
                  al.relevance DESC, al.id",
    )?;
    let rows = stmt
        .query_map([iata, owner_id], |r| {
            Ok(Airline {
                id: r.get(0)?,
                name: r.get(1)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows)
}

/// The owner's flight with this airline code, number and local departure date, if any.
fn tracked_by_code(conn: &Connection, owner_id: &str, w: &Wanted) -> Result<Option<String>> {
    let mut stmt = conn.prepare(
        "SELECT f.id, COALESCE(f.departureScheduleGateOriginal, f.lastKnownDepartureDate), ap.timeZoneIdentifier
         FROM UserFlight uf
         JOIN Flight f ON f.id = uf.flightId
         JOIN Airline al ON al.id = f.airlineId
         LEFT JOIN Airport ap ON ap.id = f.departureAirportId
         WHERE uf.userId = ?1 AND uf.deleted IS NULL AND f.deleted IS NULL
           AND al.iata = ?2 AND f.number = ?3",
    )?;
    let rows: Vec<(String, Option<i64>, Option<String>)> = stmt
        .query_map([owner_id, &w.iata, &w.number], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows.into_iter().find_map(|(id, dep, tz)| {
        let local = time::local_date(dep?, tz.as_deref())?;
        (local == w.date).then_some(id)
    }))
}

fn tracked_by_id(conn: &Connection, owner_id: &str, flight_id: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM UserFlight WHERE userId = ?1 AND flightId = ?2 AND deleted IS NULL",
            [owner_id, flight_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn already_tracked(w: &Wanted, flight_id: String) -> WriteResult {
    WriteResult {
        outcome: "already_tracked".into(),
        flight_code: Some(w.code()),
        date: Some(w.date.clone()),
        message: format!(
            "{} on {} is already in your flights ({flight_id}); pass --force to add it anyway.",
            w.code(),
            w.date
        ),
        flight_id: Some(flight_id),
        airline: None,
    }
}

fn track(
    ctx: &Ctx,
    owner_id: &str,
    w: &Wanted,
    passenger: bool,
    client: &Client,
) -> Result<WriteResult> {
    let (candidates, tracked) = {
        let conn = db::open(ctx)?;
        (
            airline_candidates(&conn, owner_id, &w.iata)?,
            tracked_by_code(&conn, owner_id, w)?,
        )
    };
    if let Some(id) = tracked.filter(|_| !w.force) {
        return Ok(already_tracked(w, id));
    }
    if candidates.is_empty() {
        return Err(Error::NotFound(format!(
            "no airline with code {} in Flighty's local data",
            w.iata
        )));
    }

    let mut found = None;
    let mut format_error = None;
    for airline in &candidates {
        match client.search(&airline.id, &w.number, &w.date) {
            Ok(Some(id)) => {
                found = Some((airline, id));
                break;
            }
            Ok(None) => {}
            // The decoder failed closed for this airline; another candidate may still match.
            Err(Error::Api(m)) if m == "unexpected search response format" => {
                format_error = Some(Error::Api(m))
            }
            Err(e) => return Err(e),
        }
    }
    let Some((airline, flight_id)) = found else {
        if let Some(e) = format_error {
            return Err(e);
        }
        let tried: Vec<&str> = candidates.iter().map(|a| a.name.as_str()).collect();
        return Err(Error::NotFound(format!(
            "no flight {} on {} (tried {}); check the flight number and date",
            w.code(),
            w.date,
            tried.join(", ")
        )));
    };

    if !w.force && tracked_by_id(&db::open(ctx)?, owner_id, &flight_id)? {
        return Ok(already_tracked(w, flight_id));
    }
    client.subscribe(&flight_id, passenger)?;
    let (outcome, verb) = if passenger {
        ("added", "Added")
    } else {
        ("followed", "Following")
    };
    Ok(WriteResult {
        outcome: outcome.into(),
        flight_code: Some(w.code()),
        date: Some(w.date.clone()),
        message: format!(
            "{verb} {} on {} on the server; the Mac app shows it after its next sync.",
            w.code(),
            w.date
        ),
        flight_id: Some(flight_id),
        airline: Some(airline.name.clone()),
    })
}

/// EXPERIMENTAL: remove a flight from the account. Needs `FLIGHTY_ALLOW_REMOVE=1` and `--yes`.
pub fn remove(ctx: &Ctx, a: &RemoveArgs) -> Result<WriteResult> {
    remove_with(ctx, a, &Client::new(ctx)?)
}

fn check_remove(ctx: &Ctx, a: &RemoveArgs) -> Result<()> {
    if !proto::is_uuid(a.id.trim()) {
        return Err(Error::BadInput(format!(
            "invalid flight id {:?}: expected a Flighty flight UUID (see `flightydeck get`)",
            a.id
        )));
    }
    ctx.guard(Class::Destructive)?;
    if !a.yes {
        return Err(Error::Refused(
            "removing a flight can't be undone here; pass --yes to confirm".into(),
        ));
    }
    Ok(())
}

pub(crate) fn remove_with(ctx: &Ctx, a: &RemoveArgs, client: &Client) -> Result<WriteResult> {
    check_remove(ctx, a)?;
    let id = a.id.trim();
    client.remove(&creds::sync_url(ctx)?, id)?;
    Ok(WriteResult {
        outcome: "removed".into(),
        flight_code: None,
        date: None,
        flight_id: Some(id.to_string()),
        airline: None,
        message: format!(
            "Removed flight {id} on the server (experimental); the Mac app drops it after its next sync."
        ),
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::api::mock::Mock;
    use crate::api::testing::{self, OWNER};

    const F01: &str = "f0000000-0000-4000-8000-000000000001";
    const NEW: &str = "11111111-2222-4333-8444-555555555555";
    const QATAR: &str = "b0000000-0000-4000-8000-000000000001";
    const QATAR_EXEC: &str = "b0000000-0000-4000-8000-000000000002";

    fn args(flight: &str, date: &str) -> AddArgs {
        AddArgs {
            flight: flight.into(),
            date: date.into(),
            force: false,
        }
    }

    fn found(uuid: &str) -> Vec<u8> {
        let mut leaf = Vec::new();
        proto::put_bytes_field(&mut leaf, 1, uuid.as_bytes());
        let mut result = Vec::new();
        proto::put_bytes_field(&mut result, 1, &leaf);
        let mut msg = Vec::new();
        proto::put_bytes_field(&mut msg, 2, &result);
        msg
    }

    fn client(ctx: &Ctx, mock: &Mock) -> Client {
        Client::for_test(ctx, &mock.base, Duration::ZERO)
    }

    #[test]
    fn parses_codes() {
        let w = validate(&args(" qr-111 ", "2031-03-17")).unwrap();
        assert_eq!((w.iata.as_str(), w.number.as_str()), ("QR", "111"));
        assert_eq!(validate(&args("9w 7", "2031-03-17")).unwrap().code(), "9W7");
        assert_eq!(
            validate(&args("u2 1234", "2031-03-17")).unwrap().code(),
            "U21234"
        );
    }

    #[test]
    fn validation_rejects_before_any_request() {
        let (_d, ctx) = testing::ctx();
        let mock = Mock::start(vec![(200, found(NEW))]);
        let c = client(&ctx, &mock);
        for (code, date) in [
            ("QR", "2031-03-17"),
            ("111", "2031-03-17"),
            ("QRX111", "2031-03-17"),
            ("", "2031-03-17"),
            ("QR111", ""),
            ("QR111", "2031-02-30"),
            ("QR111", "2031-3-7"),
            ("QR111", "17.03.2031"),
            ("QR111", "tomorrow"),
        ] {
            let e = add_for(&ctx, OWNER, &args(code, date), &c).unwrap_err();
            assert!(matches!(e, Error::BadInput(_)), "{code} {date}: {e}");
            let e = follow_for(&ctx, OWNER, &args(code, date), &c).unwrap_err();
            assert!(matches!(e, Error::BadInput(_)), "{code} {date}: {e}");
        }
        assert!(mock.requests().is_empty());
    }

    #[test]
    fn read_only_refuses_without_requests() {
        let (_d, mut ctx) = testing::ctx();
        ctx.read_only = true;
        ctx.allow_remove = true;
        let mock = Mock::start(vec![(200, found(NEW))]);
        let c = client(&ctx, &mock);
        assert!(matches!(
            add_for(&ctx, OWNER, &args("QR999", "2031-03-17"), &c),
            Err(Error::Refused(_))
        ));
        assert!(matches!(
            follow_for(&ctx, OWNER, &args("QR999", "2031-03-17"), &c),
            Err(Error::Refused(_))
        ));
        let rm = RemoveArgs {
            id: NEW.into(),
            yes: true,
        };
        assert!(matches!(remove_with(&ctx, &rm, &c), Err(Error::Refused(_))));
        assert!(mock.requests().is_empty());
    }

    #[test]
    fn candidates_prefer_used_airlines_then_relevance() {
        let (_d, ctx) = testing::ctx();
        let conn = rusqlite::Connection::open(&ctx.db_path).unwrap();
        // A more relevant QR airline the owner never flew, and a deleted one.
        conn.execute_batch(
            "INSERT INTO Airline (id,name,iata,relevance,created,lastUpdated) VALUES
               ('b0000000-0000-4000-8000-0000000000f1','Busy QR',        'QR',999,0,0);
             INSERT INTO Airline (id,name,iata,relevance,created,lastUpdated,deleted) VALUES
               ('b0000000-0000-4000-8000-0000000000f2','Gone QR','QR',5000,0,0,1);",
        )
        .unwrap();
        let ids: Vec<String> = airline_candidates(&conn, OWNER, "QR")
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(
            ids,
            [QATAR, "b0000000-0000-4000-8000-0000000000f1", QATAR_EXEC]
        );
        // For someone with no flights, relevance alone decides.
        let ids: Vec<String> = airline_candidates(&conn, "nobody", "QR")
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(
            ids,
            ["b0000000-0000-4000-8000-0000000000f1", QATAR, QATAR_EXEC]
        );
    }

    #[test]
    fn already_tracked_by_code_and_date() {
        let (_d, ctx) = testing::ctx();
        let mock = Mock::start(vec![(200, found(NEW)), (200, vec![])]);
        let c = client(&ctx, &mock);
        let r = add_for(&ctx, OWNER, &args("qr 111", "2031-03-17"), &c).unwrap();
        assert_eq!(r.outcome, "already_tracked");
        assert_eq!(r.flight_id.as_deref(), Some(F01));
        assert!(mock.requests().is_empty());

        // Someone else's flight doesn't count.
        let r = add_for(&ctx, "someone-else", &args("QR111", "2031-03-17"), &c).unwrap();
        assert_eq!(r.outcome, "added");
    }

    #[test]
    fn already_tracked_by_returned_uuid() {
        let (_d, ctx) = testing::ctx();
        // Different date locally, but the server resolves to a flight the owner already has.
        let mock = Mock::start(vec![(200, found(F01))]);
        let c = client(&ctx, &mock);
        let r = add_for(&ctx, OWNER, &args("QR111", "2031-03-18"), &c).unwrap();
        assert_eq!(r.outcome, "already_tracked");
        assert_eq!(mock.requests().len(), 1);
        assert_eq!(mock.requests()[0].target, "/v1/search");
    }

    #[test]
    fn force_adds_anyway() {
        let (_d, ctx) = testing::ctx();
        let mock = Mock::start(vec![(200, found(F01)), (200, vec![])]);
        let c = client(&ctx, &mock);
        let a = AddArgs {
            force: true,
            ..args("QR111", "2031-03-17")
        };
        let r = add_for(&ctx, OWNER, &a, &c).unwrap();
        assert_eq!(r.outcome, "added");
        assert_eq!(
            mock.requests()[1].target,
            format!("/v1/flight/{F01}/subscribe?is_passenger=true&source")
        );
    }

    #[test]
    fn tries_every_candidate_then_subscribes() {
        let (_d, ctx) = testing::ctx();
        // First candidate (Qatar Airways) finds nothing; second (Qatar Executive) does.
        let mock = Mock::start(vec![(200, vec![]), (200, found(NEW)), (200, vec![])]);
        let c = client(&ctx, &mock);
        let r = add_for(&ctx, OWNER, &args("QR999", "2031-04-01"), &c).unwrap();
        assert_eq!(r.outcome, "added");
        assert_eq!(r.flight_id.as_deref(), Some(NEW));
        assert_eq!(r.airline.as_deref(), Some("Qatar Executive"));
        assert_eq!(
            r.message,
            "Added QR999 on 2031-04-01 on the server; the Mac app shows it after its next sync."
        );
        let reqs = mock.requests();
        assert_eq!(reqs.len(), 3);
        assert_eq!(
            reqs[0].body,
            proto::search_request(QATAR, "999", "2031-04-01")
        );
        assert_eq!(
            reqs[1].body,
            proto::search_request(QATAR_EXEC, "999", "2031-04-01")
        );
        assert_eq!(
            reqs[2].target,
            format!("/v1/flight/{NEW}/subscribe?is_passenger=true&source")
        );
    }

    #[test]
    fn follow_subscribes_as_non_passenger() {
        let (_d, ctx) = testing::ctx();
        let mock = Mock::start(vec![(200, found(NEW)), (200, vec![])]);
        let c = client(&ctx, &mock);
        let r = follow_for(&ctx, OWNER, &args("QR999", "2031-04-01"), &c).unwrap();
        assert_eq!(r.outcome, "followed");
        assert_eq!(
            mock.requests()[1].target,
            format!("/v1/flight/{NEW}/subscribe?is_passenger=false&source")
        );
    }

    #[test]
    fn not_found_and_unknown_airline() {
        let (_d, ctx) = testing::ctx();
        let mock = Mock::start(vec![(200, vec![]), (200, vec![])]);
        let c = client(&ctx, &mock);
        let e = add_for(&ctx, OWNER, &args("QR999", "2031-04-01"), &c).unwrap_err();
        assert!(
            matches!(e, Error::NotFound(ref m) if m.contains("Qatar Executive")),
            "{e}"
        );
        assert_eq!(mock.requests().len(), 2);

        let e = add_for(&ctx, OWNER, &args("ZZ1", "2031-04-01"), &c).unwrap_err();
        assert!(matches!(e, Error::NotFound(_)), "{e}");
        assert_eq!(mock.requests().len(), 2);
    }

    #[test]
    fn unexpected_format_never_subscribes() {
        let (_d, ctx) = testing::ctx();
        // A UUID in the response, but not at 2.1.1.
        let mut decoy = Vec::new();
        proto::put_bytes_field(&mut decoy, 1, NEW.as_bytes());
        let mut body = Vec::new();
        proto::put_bytes_field(&mut body, 1, &decoy);
        let mock = Mock::start(vec![(200, body.clone()), (200, body)]);
        let c = client(&ctx, &mock);
        let e = add_for(&ctx, OWNER, &args("QR999", "2031-04-01"), &c).unwrap_err();
        assert!(
            matches!(e, Error::Api(ref m) if m == "unexpected search response format"),
            "{e}"
        );
        assert!(mock.requests().iter().all(|r| r.target == "/v1/search"));
    }

    #[test]
    fn remove_needs_opt_in_and_yes() {
        let (_d, mut ctx) = testing::ctx();
        let mock = Mock::start(vec![]);
        let c = client(&ctx, &mock);
        let yes = RemoveArgs {
            id: NEW.into(),
            yes: true,
        };
        let e = remove_with(&ctx, &yes, &c).unwrap_err();
        assert!(
            matches!(e, Error::Refused(ref m) if m.contains("FLIGHTY_ALLOW_REMOVE")),
            "{e}"
        );

        ctx.allow_remove = true;
        let no = RemoveArgs {
            id: NEW.into(),
            yes: false,
        };
        let e = remove_with(&ctx, &no, &c).unwrap_err();
        assert!(
            matches!(e, Error::Refused(ref m) if m.contains("--yes")),
            "{e}"
        );

        let bad = RemoveArgs {
            id: "nope".into(),
            yes: true,
        };
        assert!(matches!(
            remove_with(&ctx, &bad, &c),
            Err(Error::BadInput(_))
        ));

        // The fixture's sync URL is the real host, which a test client refuses to call.
        let e = remove_with(&ctx, &yes, &c).unwrap_err();
        assert!(matches!(e, Error::Refused(_)), "{e}");
        assert!(mock.requests().is_empty());
    }

    #[test]
    fn remove_posts_to_sync_url() {
        let (_d, mut ctx) = testing::ctx();
        ctx.allow_remove = true;
        let mock = Mock::start(vec![(200, vec![])]);
        let conn = rusqlite::Connection::open(&ctx.db_path).unwrap();
        conn.execute(
            "UPDATE SyncInfo SET nextURL = ?1",
            [format!("{}v1/sync/full?cursor=test", mock.base)],
        )
        .unwrap();
        drop(conn);
        let c = client(&ctx, &mock);
        let r = remove_with(
            &ctx,
            &RemoveArgs {
                id: NEW.into(),
                yes: true,
            },
            &c,
        )
        .unwrap();
        assert_eq!(r.outcome, "removed");
        let reqs = mock.requests();
        assert_eq!(
            reqs[0].target,
            "/v1/sync/full?cursor=test&fast_flight_sync=true"
        );
        assert_eq!(
            proto::field_path(&reqs[0].body, &[1, 11, 1]),
            Some(NEW.as_bytes())
        );
    }
}
