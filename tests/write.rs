//! Public write ops: everything here must fail before any network call. Request-level
//! behaviour (search, subscribe, already-tracked) is covered by unit tests against a local
//! mock server in `src/ops/write.rs` and `src/api.rs`.

mod common;

use flightydeck::Error;
use flightydeck::ops::write::{self, AddArgs, RemoveArgs};

const FLIGHT: &str = "11111111-2222-4333-8444-555555555555";

fn args(flight: &str, date: &str) -> AddArgs {
    AddArgs {
        flight: flight.into(),
        date: date.into(),
        force: false,
    }
}

#[test]
fn bad_input_is_rejected_first() {
    let f = common::fixture();
    let mut ctx = f.ctx();
    // Read-only too: validation still wins, so the user learns about the typo first.
    ctx.read_only = true;
    for (code, date) in [
        ("QR", "2031-03-17"),
        ("QRX1", "2031-03-17"),
        ("QR111", ""),
        ("QR111", "2031-02-29"),
    ] {
        for r in [
            write::add(&ctx, &args(code, date)),
            write::follow(&ctx, &args(code, date)),
        ] {
            let e = r.unwrap_err();
            assert!(matches!(e, Error::BadInput(_)), "{code} {date}: {e}");
            assert_eq!(e.exit_code(), 2);
        }
    }
}

#[test]
fn read_only_refuses_writes() {
    let f = common::fixture();
    let mut ctx = f.ctx();
    ctx.read_only = true;
    ctx.allow_remove = true;
    let a = args("QR111", "2031-03-17");
    assert!(matches!(write::add(&ctx, &a), Err(Error::Refused(_))));
    assert!(matches!(write::follow(&ctx, &a), Err(Error::Refused(_))));
    let rm = RemoveArgs {
        id: FLIGHT.into(),
        yes: true,
    };
    assert!(matches!(write::remove(&ctx, &rm), Err(Error::Refused(_))));
}

#[test]
fn remove_refused_without_allow_remove() {
    let f = common::fixture();
    let ctx = f.ctx();
    let e = write::remove(
        &ctx,
        &RemoveArgs {
            id: FLIGHT.into(),
            yes: true,
        },
    )
    .unwrap_err();
    assert!(
        matches!(e, Error::Refused(ref m) if m.contains("FLIGHTY_ALLOW_REMOVE")),
        "{e}"
    );
    assert_eq!(e.exit_code(), 5);
}

#[test]
fn remove_needs_yes_and_a_uuid() {
    let f = common::fixture();
    let mut ctx = f.ctx();
    ctx.allow_remove = true;
    let e = write::remove(
        &ctx,
        &RemoveArgs {
            id: FLIGHT.into(),
            yes: false,
        },
    )
    .unwrap_err();
    assert!(
        matches!(e, Error::Refused(ref m) if m.contains("--yes")),
        "{e}"
    );
    let e = write::remove(
        &ctx,
        &RemoveArgs {
            id: "QR111".into(),
            yes: true,
        },
    )
    .unwrap_err();
    assert!(matches!(e, Error::BadInput(_)), "{e}");
}

/// Live check against the real API: SEARCH ONLY, never subscribe or remove.
/// `FLIGHTY_LIVE=1 cargo test --test write -- --ignored live_search`
#[test]
#[ignore]
fn live_search_finds_a_known_flight() {
    if std::env::var("FLIGHTY_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: set FLIGHTY_LIVE=1");
        return;
    }
    let ctx = flightydeck::Ctx::from_env();
    // The most recent live flight in the real DB; nothing about it is printed.
    let (local_id, airline_id, number, dep, tz): (String, String, String, i64, Option<String>) =
        flightydeck::db::open(&ctx)
            .unwrap()
            .query_row(
                "SELECT f.id, f.airlineId, f.number, f.departureScheduleGateOriginal, ap.timeZoneIdentifier
                 FROM Flight f LEFT JOIN Airport ap ON ap.id = f.departureAirportId
                 WHERE f.deleted IS NULL AND f.departureScheduleGateOriginal IS NOT NULL
                 ORDER BY f.departureScheduleGateOriginal DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
    let date = flightydeck::time::local_date(dep, tz.as_deref()).unwrap();
    let client = flightydeck::api::Client::new(&ctx).unwrap();
    let found = client.search(&airline_id, &number, &date).unwrap();
    let uuid = found.expect("search returned no flight UUID");
    assert!(flightydeck::proto::is_uuid(&uuid));
    eprintln!(
        "live search: UUID returned; equals local Flight.id: {}",
        uuid.eq_ignore_ascii_case(&local_id)
    );
}
