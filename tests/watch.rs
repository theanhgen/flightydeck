mod common;

use common::fixture;
use flightydeck::ops::status::FlightRef;
use flightydeck::ops::watch;

fn r(flight: &str) -> FlightRef {
    FlightRef {
        flight: flight.into(),
        date: None,
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[test]
fn in_air_flight_gets_an_estimated_position() {
    let fx = fixture();
    let s = watch::snapshot(&fx.ctx(), &r("IB888"), None).unwrap();
    assert_eq!(s.phase, "in_air");
    assert!(!s.done);
    assert!(s.changes.is_empty());
    let p = s.position.clone().expect("position while in the air");
    assert!(p.estimated);
    // Left 55 minutes ago, 5 hours to go: about 15.5 % of the way from Madrid to London.
    assert!((15.0..16.5).contains(&p.progress_pct), "{}", p.progress_pct);
    let total = p.flown_km + p.remaining_km;
    assert!((1200.0..1300.0).contains(&total), "{total}");
    assert!((40.4..51.5).contains(&p.latitude), "{}", p.latitude);
    assert!((-3.6..-0.4).contains(&p.longitude), "{}", p.longitude);
    // London is north-north-east of Madrid.
    assert!((0.0..40.0).contains(&p.heading_deg), "{}", p.heading_deg);
    assert!((298..=301).contains(&p.remaining_minutes));
    assert!(s.line().contains("estimated from the schedule"));
}

#[test]
fn no_position_on_the_ground_and_done_when_over() {
    let fx = fixture();
    let ctx = fx.ctx();
    let scheduled = watch::snapshot(&ctx, &r("IB111"), None).unwrap();
    assert_eq!(scheduled.phase, "scheduled");
    assert!(scheduled.position.is_none() && !scheduled.done);
    assert_eq!(scheduled.line(), scheduled.summary);

    for code in ["LH1486", "AM7777"] {
        let s = watch::snapshot(&ctx, &r(code), None).unwrap();
        assert!(s.done && s.position.is_none(), "{code}: {}", s.phase);
    }
}

#[test]
fn position_moves_with_time_and_stops_at_arrival() {
    let fx = fixture();
    let ctx = fx.ctx();
    let at = |t: i64| watch::snapshot_at(&ctx, &r("IB888"), None, t).unwrap();
    let (early, later) = (at(now()), at(now() + 3600));
    let (a, b) = (early.position.unwrap(), later.position.unwrap());
    assert!(b.progress_pct > a.progress_pct && b.remaining_km < a.remaining_km);
    assert!(b.latitude > a.latitude, "flying north");
    // After the expected arrival the flight is no longer "in the air", so no position.
    assert!(at(now() + 6 * 3600).position.is_none());
}

#[test]
fn changes_between_readings() {
    let fx = fixture();
    let ctx = fx.ctx();
    let first = watch::snapshot(&ctx, &r("IB111"), None).unwrap();
    rusqlite::Connection::open(&fx.db)
        .unwrap()
        .execute_batch(&format!(
            "UPDATE Flight SET departureGate = 'B7',
               departureScheduleGateEstimated = departureScheduleGateOriginal + 1500
             WHERE id = '{}'",
            first.flight_id
        ))
        .unwrap();
    // The second reading follows the flight by id, as the CLI does.
    let second = watch::snapshot(&ctx, &r(&first.flight_id), Some(&first)).unwrap();
    assert_eq!(second.departure_delay_minutes, Some(25));
    assert_eq!(second.changes.len(), 2, "{:?}", second.changes);
    assert!(second.changes[0].starts_with("departure ") && second.changes[0].contains("15:15"));
    assert_eq!(second.changes[1], "departure gate A1 → B7");

    let third = watch::snapshot(&ctx, &r(&first.flight_id), Some(&second)).unwrap();
    assert!(third.changes.is_empty());
}
