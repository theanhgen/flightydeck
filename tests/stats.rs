mod common;

use flightydeck::ops::stats::{self, StatsArgs};
use flightydeck::render::Render;

use common::OWNER;

fn run(year: Option<i32>) -> stats::Stats {
    let fx = common::fixture();
    let conn = flightydeck::db::open(&fx.ctx()).unwrap();
    stats::stats_for(&conn, OWNER, &StatsArgs { year }).unwrap()
}

#[test]
fn lifetime_counts_own_flights_only() {
    let s = run(None);
    // f01-f06, f10 and the manual e01; not followed f07, friend's f08 or cancelled f09.
    assert_eq!(s.flights, 8);
    assert_eq!(s.distance_km, 18130.0);
    assert_eq!(s.airports, 6);
    assert_eq!(s.countries, 4);
    assert_eq!(s.airlines, 3);
    assert_eq!(s.aircraft_types, 1);
    assert_eq!(s.top_routes.first(), Some(&("DUS–LHR".to_string(), 2)));
    assert_eq!(s.top_airports.first(), Some(&("LHR".to_string(), 5)));
    assert_eq!(
        s.top_airlines,
        vec![
            ("Iberia".to_string(), 3),
            ("Lufthansa".to_string(), 3),
            ("Aeroméxico".to_string(), 2)
        ]
    );
    // The date is local to the departure airport: 22:10 UTC is 23:10 in Madrid.
    assert_eq!(
        s.longest.as_deref(),
        Some("IB222 MAD–MEX, 6380 km, 2031-03-17")
    );
}

#[test]
fn round_trip_counts_each_country_once() {
    // 2025: DUS–LHR twice and LHR–DUS once. Upstream counted 4 countries here.
    let s = run(Some(2025));
    assert_eq!(s.flights, 3);
    assert_eq!(s.countries, 2);
    assert_eq!(s.airports, 2);
}

#[test]
fn air_time_prefers_actual_times() {
    // f05 16:20→17:32 (72), f06 15:58→17:05 (67), manual e01 06:00→07:15 (75).
    assert_eq!(run(Some(2025)).air_minutes, 214);
}

#[test]
fn empty_year() {
    let s = run(Some(1999));
    assert_eq!(s.flights, 0);
    assert_eq!(s.longest, None);
    assert!(s.table().contains("(none)"));
}

#[test]
fn renders_table() {
    let t = run(None).table();
    assert!(t.contains("All time") && t.contains("18130 km") && t.contains("DUS–LHR"));
}

#[test]
fn public_entry_point_resolves_owner() {
    let fx = common::fixture();
    let s = stats::stats(&fx.ctx(), &StatsArgs::default()).unwrap();
    assert_eq!(s.flights, 8);
}
