//! list / get / search / current — the owner's flights.

use std::sync::LazyLock;

use regex::Regex;
use rusqlite::Connection;
use rusqlite::types::Value;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;
use crate::db::{self, col_or_null};
use crate::error::{Error, Result};
use crate::model::{AirportRef, Flight, Relation, Ticket};
use crate::owner;
use crate::render::{self, Render};
use crate::time::{self, Stamp};

/// Args structs double as clap args (CLI) and JSON-schema tool input (MCP).
#[derive(Debug, Clone, Default, Deserialize, JsonSchema, clap::Args)]
pub struct ListArgs {
    /// Only flights departing from now on.
    #[arg(long)]
    #[serde(default)]
    pub upcoming: bool,
    /// Only flights that already departed.
    #[arg(long, conflicts_with = "upcoming")]
    #[serde(default)]
    pub past: bool,
    /// Include flights archived in the app.
    #[arg(long)]
    #[serde(default)]
    pub include_archived: bool,
    /// Include flights the owner follows but doesn't fly.
    #[arg(long)]
    #[serde(default)]
    pub include_following: bool,
    /// Only flights departing in this calendar year (local date at the departure airport).
    #[arg(long)]
    pub year: Option<i32>,
    /// Maximum number of flights (default 50).
    #[arg(long)]
    pub limit: Option<u32>,
    /// Skip this many flights before applying the limit, for paging.
    #[arg(long)]
    pub offset: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema, clap::Args)]
pub struct GetArgs {
    /// Flight code ("BA286", "UA 901") or Flighty flight UUID.
    pub flight: String,
    /// Departure date YYYY-MM-DD (local). Default: nearest to now.
    #[arg(long)]
    pub date: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, clap::Args)]
pub struct SearchArgs {
    /// Free text: matches flight code, airline, airports, cities (accent-insensitive).
    pub query: Option<String>,
    /// Departure airport IATA or city.
    #[arg(long)]
    pub from: Option<String>,
    /// Arrival airport IATA or city.
    #[arg(long)]
    pub to: Option<String>,
    /// Airline IATA code or name.
    #[arg(long)]
    pub airline: Option<String>,
    /// Earliest departure date YYYY-MM-DD.
    #[arg(long)]
    pub after: Option<String>,
    /// Latest departure date YYYY-MM-DD.
    #[arg(long)]
    pub before: Option<String>,
    /// Maximum results (default 50).
    #[arg(long)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, clap::Args)]
pub struct CurrentArgs {
    /// Also include flights departing within this many hours (default 6).
    #[arg(long)]
    pub ahead_hours: Option<u32>,
    /// Also include flights landed within this many hours (default 2).
    #[arg(long)]
    pub landed_hours: Option<u32>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FlightList {
    pub owner_user_id: String,
    pub count: usize,
    pub flights: Vec<Flight>,
}

const DEFAULT_LIMIT: u32 = 50;

// ---------------------------------------------------------------------------------------------
// Rendering

impl Render for FlightList {
    fn table(&self) -> String {
        let rows: Vec<Vec<String>> = self
            .flights
            .iter()
            .map(|f| {
                vec![
                    render::dash(f.departure_scheduled.as_ref().map(Stamp::human)),
                    f.flight_code.clone(),
                    route(f),
                    render::dash(f.duration_minutes.map(time::duration)),
                    notes(f),
                ]
            })
            .collect();
        render::table(
            &["Departure", "Flight", "Route", "Duration", "Status"],
            &rows,
        )
    }
}

impl Render for Flight {
    fn table(&self) -> String {
        let stamp = |s: &Option<Stamp>| s.as_ref().map(Stamp::human);
        let place = |t: &Option<String>, g: &Option<String>| match (t, g) {
            (None, None) => None,
            (t, g) => Some(format!(
                "terminal {} gate {}",
                render::dash(t.as_deref()),
                render::dash(g.as_deref())
            )),
        };
        let mut lines: Vec<(&str, Option<String>)> = vec![
            ("Flight", Some(title(self))),
            ("Route", Some(route(self))),
            ("Status", Some(notes(self)).filter(|s| !s.is_empty())),
            ("Departure", stamp(&self.departure_scheduled)),
            ("  estimated", stamp(&self.departure_estimated)),
            ("  actual", stamp(&self.departure_actual)),
            (
                "  from",
                place(&self.departure_terminal, &self.departure_gate),
            ),
            ("Arrival", stamp(&self.arrival_scheduled)),
            ("  estimated", stamp(&self.arrival_estimated)),
            ("  actual", stamp(&self.arrival_actual)),
            ("  at", place(&self.arrival_terminal, &self.arrival_gate)),
            ("  baggage belt", self.baggage_belt.clone()),
            ("Duration", self.duration_minutes.map(time::duration)),
            ("Aircraft", self.aircraft.clone()),
            ("Tail", self.tail_number.clone()),
        ];
        if let Some(t) = &self.ticket {
            lines.push(("Booking", t.booking_reference.clone()));
            lines.push(("Seat", t.seat.clone()));
            lines.push(("Cabin", t.cabin.clone()));
        }
        lines.push(("Id", Some(self.id.clone())));
        let width = lines.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        lines
            .into_iter()
            .filter_map(|(k, v)| v.map(|v| format!("{k:<width$}  {v}")))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn title(f: &Flight) -> String {
    match &f.airline_name {
        Some(name) => format!("{} {name}", f.flight_code),
        None => f.flight_code.clone(),
    }
}

fn airport_label(a: &AirportRef) -> String {
    a.iata
        .clone()
        .or_else(|| a.icao.clone())
        .or_else(|| a.name.clone())
        .unwrap_or_else(|| "?".into())
}

fn route(f: &Flight) -> String {
    let mut s = format!("{} → {}", airport_label(&f.from), airport_label(&f.to));
    if let Some(d) = &f.diverted_to {
        s.push_str(&format!(" (landed {})", airport_label(d)));
    }
    s
}

/// "[CANCELLED]", "12 min late", "(friend: Alex)" and similar, space-separated.
fn notes(f: &Flight) -> String {
    let mut n = Vec::new();
    if f.cancelled {
        n.push("[CANCELLED]".to_string());
    }
    if f.diverted {
        n.push("[DIVERTED]".to_string());
    }
    if !f.cancelled {
        if let Some(d) = departure_delay_seconds(f) {
            n.push(format!("dep {}", time::delay(d)));
        }
        if let Some(d) = arrival_delay_seconds(f) {
            n.push(format!("arr {}", time::delay(d)));
        }
    }
    match f.relation {
        Relation::Mine => {}
        Relation::Following => n.push("(following)".into()),
        Relation::Friend => n.push(format!(
            "(friend: {})",
            f.friend_name.as_deref().unwrap_or("?")
        )),
    }
    if f.manual {
        n.push("(manual)".into());
    }
    n.join(" ")
}

// ---------------------------------------------------------------------------------------------
// Time helpers shared with status.rs

/// Best known departure: actual, else estimated, else scheduled.
pub(crate) fn departure_time(f: &Flight) -> Option<i64> {
    [
        &f.departure_actual,
        &f.departure_estimated,
        &f.departure_scheduled,
    ]
    .into_iter()
    .find_map(|s| s.as_ref().map(|s| s.unix))
}

/// Best known arrival: actual, else estimated, else scheduled.
pub(crate) fn arrival_time(f: &Flight) -> Option<i64> {
    [
        &f.arrival_actual,
        &f.arrival_estimated,
        &f.arrival_scheduled,
    ]
    .into_iter()
    .find_map(|s| s.as_ref().map(|s| s.unix))
}

/// Seconds between the scheduled and the actual (or estimated) departure.
pub(crate) fn departure_delay_seconds(f: &Flight) -> Option<i64> {
    let best = f
        .departure_actual
        .as_ref()
        .or(f.departure_estimated.as_ref())?;
    Some(best.unix - f.departure_scheduled.as_ref()?.unix)
}

pub(crate) fn arrival_delay_seconds(f: &Flight) -> Option<i64> {
    let best = f.arrival_actual.as_ref().or(f.arrival_estimated.as_ref())?;
    Some(best.unix - f.arrival_scheduled.as_ref()?.unix)
}

fn sort_key(f: &Flight) -> i64 {
    f.departure_scheduled
        .as_ref()
        .map(|s| s.unix)
        .or_else(|| departure_time(f))
        .unwrap_or(0)
}

/// Local calendar date of the scheduled departure at the departure airport.
fn departure_date(f: &Flight) -> Option<String> {
    let unix = f
        .departure_scheduled
        .as_ref()
        .map(|s| s.unix)
        .or_else(|| departure_time(f))?;
    time::local_date(unix, f.from.tz.as_deref())
}

/// Local calendar year of the scheduled departure, the same rule `stats --year` uses.
fn departure_year(f: &Flight) -> Option<i32> {
    let unix = f
        .departure_scheduled
        .as_ref()
        .map(|s| s.unix)
        .or_else(|| departure_time(f))?;
    time::local_year(unix, f.from.tz.as_deref())
}

// ---------------------------------------------------------------------------------------------
// Loader

/// Which user-flight rows to load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// The owner's rows; `following` adds flights the owner follows (`isMyFlight = 0`).
    Own { following: bool },
    /// Flights connected friends fly themselves.
    Friends,
}

/// Extra SQL conditions over the loader's flat columns (see `load`), with named params.
#[derive(Debug, Default)]
pub(crate) struct Filter {
    pub conditions: Vec<String>,
    pub params: Vec<(String, Value)>,
}

impl Filter {
    pub fn param(&mut self, name: &str, v: impl Into<Value>) {
        self.params.push((format!(":{name}"), v.into()));
    }
}

/// Connected friends' user ids, relative to the owner (`:owner`).
const FRIEND_IDS: &str = "SELECT CASE WHEN c.senderUserId = :owner THEN c.receiverUserId ELSE c.senderUserId END \
     FROM ConnectedFriendRelationship c \
     WHERE (c.senderUserId = :owner OR c.receiverUserId = :owner) AND c.deleted IS NULL";

fn scope_sql(scope: Scope, u: &str) -> String {
    match scope {
        Scope::Own { following: true } => format!("{u}.userId = :owner"),
        Scope::Own { following: false } => format!("{u}.userId = :owner AND {u}.isMyFlight = 1"),
        Scope::Friends => format!("{u}.userId IN ({FRIEND_IDS}) AND {u}.isMyFlight = 1"),
    }
}

/// Flight ∪ ManualFlight joined to the user's rows, airports, airline, the owner's ticket and
/// the friend's name. Deleted rows are skipped. Sorted by scheduled departure.
///
/// Flat columns usable in `Filter` conditions: `code`, `number`, `airline_iata`, `airline_name`,
/// `dep_iata`, `dep_name`, `dep_city`, `arr_iata`, `arr_name`, `arr_city`, `user_id`.
pub(crate) fn load(
    conn: &Connection,
    owner: &str,
    scope: Scope,
    filter: &Filter,
) -> Result<Vec<Flight>> {
    let f = |alias: &str, table: &str, cols: &[&str]| col_or_null(conn, table, alias, cols);
    let flight = |cols: &[&str]| f("f", "Flight", cols);
    let manual = |cols: &[&str]| f("m", "ManualFlight", cols);

    // Columns shared by both flight tables; ManualFlight has no *Estimated.
    let shared = [
        ("dep_sched", "departureScheduleGateOriginal"),
        ("dep_act", "departureScheduleGateActual"),
        ("takeoff_act", "departureScheduleRunwayActual"),
        ("arr_sched", "arrivalScheduleGateOriginal"),
        ("arr_act", "arrivalScheduleGateActual"),
        ("landing_act", "arrivalScheduleRunwayActual"),
        ("dep_terminal", "departureTerminal"),
        ("dep_gate", "departureGate"),
        ("arr_terminal", "arrivalTerminal"),
        ("arr_gate", "arrivalGate"),
        ("belt", "arrivalBaggageBelt"),
        ("cancelled", "isCancelled"),
        ("aircraft", "equipmentModelName"),
        ("tail", "equipmentTailNumber"),
        ("dep_known", "lastKnownDepartureDate"),
    ];
    let mut f_cols = Vec::new();
    let mut m_cols = Vec::new();
    for (alias, col) in shared {
        f_cols.push(format!("{} AS {alias}", flight(&[col])?));
        m_cols.push(format!("{} AS {alias}", manual(&[col])?));
    }
    f_cols.push(format!(
        "{} AS dep_est",
        flight(&["departureScheduleGateEstimated"])?
    ));
    m_cols.push(format!(
        "{} AS dep_est",
        manual(&["departureScheduleGateEstimated"])?
    ));
    f_cols.push(format!(
        "{} AS arr_est",
        flight(&["arrivalScheduleGateEstimated"])?
    ));
    m_cols.push(format!(
        "{} AS arr_est",
        manual(&["arrivalScheduleGateEstimated"])?
    ));
    f_cols.push("NULL AS manual_minutes".into());
    m_cols.push(format!(
        "{} AS manual_minutes",
        manual(&["durationMinutes"])?
    ));

    let branch = |table: &str,
                  alias: &str,
                  link: &str,
                  manual_flag: u8,
                  cols: &[String]|
     -> Result<String> {
        Ok(format!(
            "SELECT {alias}.id AS id, {manual_flag} AS manual, {alias}.number AS number, u.userId AS user_id, \
               u.isMyFlight AS is_mine, {archived} AS archived, \
               {alias}.departureAirportId AS dep_id, {alias}.scheduledArrivalAirportId AS arr_id, \
               {alias}.actualArrivalAirportId AS act_arr_id, {alias}.airlineId AS airline_id, {cols} \
             FROM {link} u JOIN {table} {alias} ON {alias}.id = u.flightId \
             WHERE {u_deleted} IS NULL AND {deleted} IS NULL AND {scope}",
            archived = col_or_null(conn, link, "u", &["isArchived"])?,
            u_deleted = col_or_null(conn, link, "u", &["deleted"])?,
            deleted = col_or_null(conn, table, alias, &["deleted"])?,
            scope = scope_sql(scope, "u"),
            cols = cols.join(", "),
        ))
    };
    let flights = branch("Flight", "f", "UserFlight", 0, &f_cols)?;
    let manuals = branch("ManualFlight", "m", "UserManualFlight", 1, &m_cols)?;

    let airport = |a: &str| -> Result<String> {
        Ok(format!(
            "{a}.iata AS {a}_iata, {icao} AS {a}_icao, {a}.name AS {a}_name, {a}.city AS {a}_city, \
             {country} AS {a}_country, {tz} AS {a}_tz",
            icao = col_or_null(conn, "Airport", a, &["icao"])?,
            country = col_or_null(conn, "Airport", a, &["country"])?,
            tz = col_or_null(conn, "Airport", a, &["timeZoneIdentifier"])?,
        ))
    };
    let t = |cols: &[&str]| col_or_null(conn, "Ticket", "t", cols);
    let sql = format!(
        "SELECT * FROM ( \
           SELECT r.*, {dep}, {arr}, {act}, \
             al.iata AS airline_iata, {al_icao} AS airline_icao, al.name AS airline_name, \
             upper(COALESCE(al.iata, {al_icao}, '')) || COALESCE(r.number, '') AS code, \
             {pnr} AS pnr, {seat} AS seat, {cabin} AS cabin, \
             CASE WHEN r.user_id = :owner THEN NULL ELSE COALESCE( \
               (SELECT p.fullName FROM Profile p WHERE p.userId = r.user_id LIMIT 1), \
               (SELECT CASE WHEN c.senderUserId = r.user_id THEN c.senderFirstName ELSE c.receiverFirstName END \
                  FROM ConnectedFriendRelationship c \
                  WHERE c.senderUserId = r.user_id OR c.receiverUserId = r.user_id LIMIT 1)) \
             END AS friend_name \
           FROM ({flights} UNION ALL {manuals}) r \
           LEFT JOIN Airport dep ON dep.id = r.dep_id \
           LEFT JOIN Airport arr ON arr.id = r.arr_id \
           LEFT JOIN Airport act ON act.id = r.act_arr_id \
           LEFT JOIN Airline al ON al.id = r.airline_id \
           LEFT JOIN Ticket t ON t.flightId = r.id AND t.userId = :owner AND {t_deleted} IS NULL \
         ) WHERE {where_sql} \
         ORDER BY COALESCE(dep_sched, dep_known), id",
        dep = airport("dep")?,
        arr = airport("arr")?,
        act = airport("act")?,
        al_icao = col_or_null(conn, "Airline", "al", &["icao"])?,
        pnr = t(&["pnr"])?,
        seat = t(&["seatNumber"])?,
        cabin = t(&["cabinClass"])?,
        t_deleted = t(&["deleted"])?,
        where_sql = if filter.conditions.is_empty() {
            "1".to_string()
        } else {
            filter.conditions.join(" AND ")
        },
    );

    let mut params: Vec<(&str, &dyn rusqlite::ToSql)> = vec![(":owner", &owner)];
    for (name, v) in &filter.params {
        params.push((name.as_str(), v));
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params.as_slice(), |r| map_row(r, owner))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

fn map_row(r: &rusqlite::Row, owner: &str) -> rusqlite::Result<Flight> {
    let text = |c: &str| -> rusqlite::Result<Option<String>> {
        Ok(r.get::<_, Option<String>>(c)?
            .filter(|s| !s.trim().is_empty()))
    };
    let int = |c: &str| r.get::<_, Option<i64>>(c);
    let airport = |p: &str| -> rusqlite::Result<AirportRef> {
        Ok(AirportRef {
            iata: text(&format!("{p}_iata"))?,
            icao: text(&format!("{p}_icao"))?,
            name: text(&format!("{p}_name"))?,
            city: text(&format!("{p}_city"))?,
            country: text(&format!("{p}_country"))?,
            tz: text(&format!("{p}_tz"))?,
        })
    };
    let from = airport("dep")?;
    let to = airport("arr")?;
    let stamp = |c: &str, tz: &AirportRef| -> rusqlite::Result<Option<Stamp>> {
        Ok(Stamp::opt(int(c)?, tz.tz.as_deref()))
    };

    let arr_id: Option<String> = r.get("arr_id")?;
    let act_arr_id: Option<String> = r.get("act_arr_id")?;
    let diverted = matches!((&arr_id, &act_arr_id), (Some(a), Some(b)) if !b.is_empty() && a != b);

    let number = text("number")?.unwrap_or_default();
    let airline_iata = text("airline_iata")?;
    let airline_code = airline_iata.clone().or(text("airline_icao")?);
    let user_id: String = r.get("user_id")?;
    let relation = if user_id != owner {
        Relation::Friend
    } else if int("is_mine")? == Some(1) {
        Relation::Mine
    } else {
        Relation::Following
    };

    let ticket = Ticket {
        booking_reference: text("pnr")?,
        seat: text("seat")?,
        cabin: text("cabin")?,
    };
    let has_ticket =
        ticket.booking_reference.is_some() || ticket.seat.is_some() || ticket.cabin.is_some();

    let mut flight = Flight {
        id: r.get("id")?,
        manual: int("manual")? == Some(1),
        flight_code: format!(
            "{}{number}",
            airline_code.unwrap_or_default().to_uppercase()
        ),
        airline_iata,
        airline_name: text("airline_name")?,
        number,
        departure_scheduled: stamp("dep_sched", &from)?,
        departure_estimated: stamp("dep_est", &from)?,
        departure_actual: stamp("dep_act", &from)?,
        arrival_scheduled: stamp("arr_sched", &to)?,
        arrival_estimated: stamp("arr_est", &to)?,
        arrival_actual: stamp("arr_act", &to)?,
        takeoff_actual: stamp("takeoff_act", &from)?,
        landing_actual: stamp("landing_act", &to)?,
        departure_terminal: text("dep_terminal")?,
        departure_gate: text("dep_gate")?,
        arrival_terminal: text("arr_terminal")?,
        arrival_gate: text("arr_gate")?,
        baggage_belt: text("belt")?,
        cancelled: int("cancelled")?.unwrap_or(0) != 0,
        diverted,
        diverted_to: if diverted {
            Some(airport("act")?)
        } else {
            None
        },
        aircraft: text("aircraft")?,
        tail_number: text("tail")?,
        duration_minutes: None,
        relation,
        friend_name: text("friend_name")?,
        archived: int("archived")?.unwrap_or(0) != 0,
        ticket: has_ticket.then_some(ticket),
        from,
        to,
    };
    if flight.departure_scheduled.is_none() {
        flight.departure_scheduled = stamp("dep_known", &flight.from)?;
    }
    flight.duration_minutes = int("manual_minutes")?.filter(|m| *m > 0).or_else(|| {
        let (d, a) = (departure_time(&flight)?, arrival_time(&flight)?);
        (a > d).then_some((a - d) / 60)
    });
    Ok(flight)
}

// ---------------------------------------------------------------------------------------------
// Lookup by code / id (shared with status.rs)

static UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}$")
        .unwrap()
});
static CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Z]{2}|\d[A-Z]|[A-Z]\d)(\d+)$").unwrap());

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FlightQuery {
    Id(String),
    Code { airline: String, number: String },
    Number(String),
}

fn strip_zeros(n: &str) -> String {
    let t = n.trim_start_matches('0');
    if t.is_empty() { "0".into() } else { t.into() }
}

/// "ib 111", "IB-111", "IB0111" → Code(QR, 111); "1486" → Number; a UUID → Id.
pub(crate) fn parse_flight(input: &str) -> Result<FlightQuery> {
    let input = input.trim();
    if UUID.is_match(input) {
        return Ok(FlightQuery::Id(input.to_lowercase()));
    }
    let norm: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(char::to_uppercase)
        .collect();
    if !norm.is_empty() && norm.chars().all(|c| c.is_ascii_digit()) {
        return Ok(FlightQuery::Number(strip_zeros(&norm)));
    }
    let caps = CODE.captures(&norm).ok_or_else(|| {
        Error::BadInput(format!(
            "'{input}' is not a flight code (like BA286 or UA 901) or a Flighty flight id"
        ))
    })?;
    Ok(FlightQuery::Code {
        airline: caps[1].to_string(),
        number: strip_zeros(&caps[2]),
    })
}

pub(crate) fn parse_date(s: &str) -> Result<String> {
    chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .map(|d| d.format("%Y-%m-%d").to_string())
        .map_err(|_| Error::BadInput(format!("'{s}' is not a date; use YYYY-MM-DD")))
}

/// Resolve a flight among the owner's and followed flights (archived included).
/// `date` picks the local departure date exactly; otherwise the nearest flight that hasn't
/// landed yet, else the most recent past one.
pub(crate) fn find(
    conn: &Connection,
    owner: &str,
    flight: &str,
    date: Option<&str>,
) -> Result<Flight> {
    let query = parse_flight(flight)?;
    let date = date.map(parse_date).transpose()?;
    let all = load(
        conn,
        owner,
        Scope::Own { following: true },
        &Filter::default(),
    )?;

    let matches: Vec<Flight> = all
        .into_iter()
        .filter(|f| match &query {
            FlightQuery::Id(id) => f.id.eq_ignore_ascii_case(id),
            FlightQuery::Code { airline, number } => {
                strip_zeros(&f.number) == *number
                    && f.flight_code.strip_suffix(f.number.as_str()) == Some(airline.as_str())
            }
            FlightQuery::Number(n) => strip_zeros(&f.number) == *n,
        })
        .collect();

    if matches.is_empty() {
        return Err(Error::NotFound(format!(
            "no flight '{}' among your flights",
            flight.trim()
        )));
    }
    if let FlightQuery::Number(n) = &query {
        let mut codes: Vec<&str> = matches.iter().map(|f| f.flight_code.as_str()).collect();
        codes.sort();
        codes.dedup();
        if codes.len() > 1 {
            return Err(Error::BadInput(format!(
                "flight number {n} is ambiguous: {}; add the airline code",
                codes.join(", ")
            )));
        }
    }

    if let Some(date) = date {
        let dates: Vec<String> = matches.iter().filter_map(departure_date).collect();
        return matches
            .into_iter()
            .find(|f| departure_date(f).as_deref() == Some(date.as_str()))
            .ok_or_else(|| {
                let mut dates = dates;
                dates.sort();
                dates.dedup();
                Error::NotFound(format!(
                    "no '{}' departing on {date}; known dates: {}",
                    flight.trim(),
                    dates.join(", ")
                ))
            });
    }

    let now = time::now_unix();
    let (open, done): (Vec<Flight>, Vec<Flight>) = matches.into_iter().partition(|f| {
        arrival_time(f)
            .or_else(|| departure_time(f))
            .is_some_and(|t| t >= now)
    });
    let nearest = open.into_iter().min_by_key(sort_key);
    Ok(nearest
        .or_else(|| done.into_iter().max_by_key(sort_key))
        .expect("matches is non-empty"))
}

// ---------------------------------------------------------------------------------------------
// Ops

fn with_conn<T>(ctx: &Ctx, f: impl FnOnce(&Connection, &str) -> Result<T>) -> Result<T> {
    let conn = db::open(ctx)?;
    let owner = owner::resolve(ctx, &conn)?;
    f(&conn, &owner.user_id)
}

/// Keep `limit` flights: the first ones for upcoming lists, the most recent otherwise.
pub(crate) fn apply_limit(
    mut flights: Vec<Flight>,
    limit: Option<u32>,
    from_start: bool,
) -> Vec<Flight> {
    let n = limit.unwrap_or(DEFAULT_LIMIT) as usize;
    if flights.len() > n {
        if from_start {
            flights.truncate(n);
        } else {
            flights.drain(..flights.len() - n);
        }
    }
    flights
}

/// Skip `offset` flights from the end `apply_limit` keeps: the first ones for upcoming lists,
/// the most recent otherwise.
fn apply_offset(mut flights: Vec<Flight>, offset: Option<u32>, from_start: bool) -> Vec<Flight> {
    let n = (offset.unwrap_or(0) as usize).min(flights.len());
    if from_start {
        flights.drain(..n);
    } else {
        flights.truncate(flights.len() - n);
    }
    flights
}

pub(crate) fn flight_list(owner: &str, flights: Vec<Flight>) -> FlightList {
    FlightList {
        owner_user_id: owner.to_string(),
        count: flights.len(),
        flights,
    }
}

/// Old app versions archive every past flight automatically. If ≥ 90 % of the owner's past
/// flights are archived, the flag carries no meaning and is ignored.
fn archive_flag_meaningful(flights: &[Flight], now: i64) -> bool {
    let past: Vec<&Flight> = flights
        .iter()
        .filter(|f| f.relation == Relation::Mine && departure_time(f).is_some_and(|t| t < now))
        .collect();
    let archived = past.iter().filter(|f| f.archived).count();
    past.is_empty() || archived * 10 < past.len() * 9
}

pub fn list(ctx: &Ctx, a: &ListArgs) -> Result<FlightList> {
    with_conn(ctx, |conn, owner| {
        let now = time::now_unix();
        let all = load(
            conn,
            owner,
            Scope::Own {
                following: a.include_following,
            },
            &Filter::default(),
        )?;
        let hide_archived = !a.include_archived && archive_flag_meaningful(&all, now);
        let flights: Vec<Flight> = all
            .into_iter()
            .filter(|f| !(hide_archived && f.archived))
            .filter(|f| {
                let t = departure_time(f).unwrap_or(0);
                (!a.upcoming || t >= now) && (!a.past || t < now)
            })
            .filter(|f| a.year.is_none() || departure_year(f) == a.year)
            .collect();
        let flights = apply_offset(flights, a.offset, a.upcoming);
        Ok(flight_list(
            owner,
            apply_limit(flights, a.limit, a.upcoming),
        ))
    })
}

pub fn get(ctx: &Ctx, a: &GetArgs) -> Result<Flight> {
    with_conn(ctx, |conn, owner| {
        find(conn, owner, &a.flight, a.date.as_deref())
    })
}

/// `LIKE` pattern for a folded substring, with `%`, `_` and `\` escaped.
fn like(s: &str) -> String {
    let esc = db::fold(s.trim())
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{esc}%")
}

fn place_filter(filter: &mut Filter, side: &str, name: &str, value: &str) {
    filter.conditions.push(format!(
        "(lower({side}_iata) = :{name} OR fold({side}_city) LIKE :{name}_like ESCAPE '\\' \
          OR fold({side}_name) LIKE :{name}_like ESCAPE '\\')"
    ));
    filter.param(name, value.trim().to_lowercase());
    filter.param(&format!("{name}_like"), like(value));
}

pub fn search(ctx: &Ctx, a: &SearchArgs) -> Result<FlightList> {
    let after = a.after.as_deref().map(parse_date).transpose()?;
    let before = a.before.as_deref().map(parse_date).transpose()?;
    let mut filter = Filter::default();
    if let Some(q) = a.query.as_deref().filter(|q| !q.trim().is_empty()) {
        let compact: String = q
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .collect();
        filter.conditions.push(
            "(fold(code) LIKE :q_code ESCAPE '\\' OR fold(airline_name) LIKE :q ESCAPE '\\' \
              OR fold(dep_iata) LIKE :q ESCAPE '\\' OR fold(dep_name) LIKE :q ESCAPE '\\' \
              OR fold(dep_city) LIKE :q ESCAPE '\\' OR fold(arr_iata) LIKE :q ESCAPE '\\' \
              OR fold(arr_name) LIKE :q ESCAPE '\\' OR fold(arr_city) LIKE :q ESCAPE '\\')"
                .into(),
        );
        filter.param("q", like(q));
        filter.param("q_code", like(&compact));
    }
    if let Some(v) = a.from.as_deref().filter(|v| !v.trim().is_empty()) {
        place_filter(&mut filter, "dep", "from", v);
    }
    if let Some(v) = a.to.as_deref().filter(|v| !v.trim().is_empty()) {
        place_filter(&mut filter, "arr", "to", v);
    }
    if let Some(v) = a.airline.as_deref().filter(|v| !v.trim().is_empty()) {
        filter.conditions.push(
            "(lower(airline_iata) = :al OR fold(airline_name) LIKE :al_like ESCAPE '\\')".into(),
        );
        filter.param("al", v.trim().to_lowercase());
        filter.param("al_like", like(v));
    }

    with_conn(ctx, |conn, owner| {
        let flights: Vec<Flight> = load(conn, owner, Scope::Own { following: true }, &filter)?
            .into_iter()
            .filter(|f| {
                let d = departure_date(f).unwrap_or_default();
                after.as_ref().is_none_or(|x| d >= *x) && before.as_ref().is_none_or(|x| d <= *x)
            })
            .collect();
        Ok(flight_list(owner, apply_limit(flights, a.limit, false)))
    })
}

pub fn current(ctx: &Ctx, a: &CurrentArgs) -> Result<FlightList> {
    let ahead = i64::from(a.ahead_hours.unwrap_or(6)) * 3600;
    let landed = i64::from(a.landed_hours.unwrap_or(2)) * 3600;
    with_conn(ctx, |conn, owner| {
        let now = time::now_unix();
        let flights = load(
            conn,
            owner,
            Scope::Own { following: true },
            &Filter::default(),
        )?
        .into_iter()
        .filter(|f| {
            let (Some(dep), Some(arr)) = (departure_time(f), arrival_time(f)) else {
                return false;
            };
            let airborne = !f.cancelled && dep <= now && arr > now;
            let departing_soon = dep > now && dep <= now + ahead;
            let just_landed = !f.cancelled && arr <= now && arr >= now - landed;
            airborne || departing_soon || just_landed
        })
        .collect();
        Ok(flight_list(owner, flights))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codes() {
        let code = |a: &str, n: &str| FlightQuery::Code {
            airline: a.into(),
            number: n.into(),
        };
        assert_eq!(parse_flight("ib 111").unwrap(), code("IB", "111"));
        assert_eq!(parse_flight("IB-0111").unwrap(), code("IB", "111"));
        assert_eq!(parse_flight("5J 123").unwrap(), code("5J", "123"));
        assert_eq!(
            parse_flight("0486").unwrap(),
            FlightQuery::Number("486".into())
        );
        assert!(matches!(
            parse_flight("F0000000-0000-4000-8000-000000000001").unwrap(),
            FlightQuery::Id(id) if id == "f0000000-0000-4000-8000-000000000001"
        ));
        assert!(parse_flight("QRX111").is_err());
        assert!(parse_flight("").is_err());
    }

    #[test]
    fn escapes_like() {
        assert_eq!(like("Düsseldorf 50%"), "%dusseldorf 50\\%%");
    }
}
