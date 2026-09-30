mod common;

use common::fixture;
use flightydeck::ops::status::{self, FlightRef};
use flightydeck::render::Render;

fn r(flight: &str, date: Option<&str>) -> FlightRef {
    FlightRef {
        flight: flight.into(),
        date: date.map(Into::into),
    }
}

#[test]
fn in_air_flight() {
    let fx = fixture();
    let s = status::flight_status(&fx.ctx(), &r("QR888", None)).unwrap();
    assert_eq!(s.phase, "in_air");
    assert_eq!(s.departure_delay_minutes, Some(5));
    assert_eq!(s.arrival_delay_minutes, Some(0));
    assert!(
        s.summary.contains("in the air") && s.summary.contains("5 min late"),
        "{}",
        s.summary
    );
    assert!(s.table().contains("QR888"));
}

#[test]
fn phases_scheduled_landed_cancelled() {
    let fx = fixture();
    let ctx = fx.ctx();
    let phase = |f: &str, d: Option<&str>| status::flight_status(&ctx, &r(f, d)).unwrap().phase;
    assert_eq!(phase("QR111", None), "scheduled");
    assert_eq!(phase("LX1486", Some("2025-05-01")), "landed");
    assert_eq!(phase("VN7777", None), "cancelled");
    assert_eq!(
        phase("LX1480", None),
        "landed",
        "manual past flight without live data"
    );
    let s = status::flight_status(&ctx, &r("LX1486", Some("2025-05-01"))).unwrap();
    assert_eq!(
        (s.departure_delay_minutes, s.arrival_delay_minutes),
        (Some(20), Some(22))
    );
}

#[test]
fn diverted_flight() {
    let fx = fixture();
    rusqlite::Connection::open(&fx.db)
        .unwrap()
        .execute_batch(
            "UPDATE Flight SET actualArrivalAirportId = 'a0000000-0000-4000-8000-000000000001' \
             WHERE id = 'f0000000-0000-4000-8000-000000000003';",
        )
        .unwrap();
    let s = status::flight_status(&fx.ctx(), &r("VN333", None)).unwrap();
    assert_eq!(s.phase, "diverted");
    assert_eq!(
        s.flight
            .diverted_to
            .as_ref()
            .and_then(|a| a.iata.as_deref()),
        Some("PRG")
    );
}

#[test]
fn forecast_from_local_history() {
    let fx = fixture();
    let f = status::delay_forecast(&fx.ctx(), &r("LX1486", None)).unwrap();
    assert_eq!(f.flight_code, "LX1486");
    assert_eq!(f.samples, 2);
    assert_eq!(f.source, "local_history");
    assert_eq!(f.on_time_pct, Some(50.0));
    assert_eq!(f.avg_delay_minutes, Some(8.5));
    assert_eq!(f.max_delay_minutes, Some(22));
    assert!(f.table().contains("50%"));
}

#[test]
fn forecast_not_enough_history() {
    let fx = fixture();
    let f = status::delay_forecast(&fx.ctx(), &r("QR111", None)).unwrap();
    assert_eq!(f.samples, 0);
    assert!(f.note.contains("not enough history"), "{}", f.note);
    assert_eq!(f.on_time_pct, None);
}

#[test]
fn forecast_prefers_flighty_columns() {
    let fx = fixture();
    rusqlite::Connection::open(&fx.db)
        .unwrap()
        .execute_batch(
            "UPDATE Flight SET delayForecastObservations = 40, delayForecastDelayMean = 12, \
               delayForecastEarlyCount = 10, delayForecastOntimeCount = 20, delayForecastLate15Count = 6, \
               delayForecastLate30Count = 2, delayForecastLate45Count = 1, delayForecastCanceledCount = 1, \
               delayForecastDivertedCount = 0 \
             WHERE id = 'f0000000-0000-4000-8000-000000000001';",
        )
        .unwrap();
    let f = status::delay_forecast(&fx.ctx(), &r("QR111", None)).unwrap();
    assert_eq!(f.source, "flighty");
    assert_eq!(f.samples, 40);
    assert_eq!(f.on_time_pct, Some(75.0));
    assert_eq!(f.avg_delay_minutes, Some(12.0));
}
