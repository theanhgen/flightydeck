//! `watch`: one flight re-read from the local database on an interval. CLI only.
//!
//! The database holds no live position, so while a flight is in the air the position is an
//! estimate: a point on the great-circle route, placed by elapsed time. It is always labelled.

use std::time::Duration;

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use crate::ctx::Ctx;
use crate::error::{Error, Result};
use crate::model::Flight;
use crate::ops::flights::{arrival_time, departure_time};
use crate::ops::status::{self, FlightRef, FlightStatus};
use crate::time::{self, Stamp};

#[derive(Debug, Clone, clap::Args)]
pub struct WatchArgs {
    /// Flight code ("BA286") or Flighty flight UUID.
    pub flight: String,
    /// Departure date YYYY-MM-DD (local). Default: nearest to now.
    #[arg(long)]
    pub date: Option<String>,
    /// Time between updates: 1m to 24h, e.g. 90s, 5m, 1h. A bare number is minutes.
    #[arg(long, default_value = "5m")]
    pub every: String,
    /// Print one update and exit.
    #[arg(long)]
    pub once: bool,
}

const MIN_INTERVAL: Duration = Duration::from_secs(60);
const MAX_INTERVAL: Duration = Duration::from_secs(24 * 3600);
const EARTH_RADIUS_KM: f64 = 6371.0;

/// "90s", "5m", "1h" or a bare number of minutes; between one minute and one day.
pub fn parse_interval(s: &str) -> Result<Duration> {
    let bad = || {
        Error::BadInput(format!(
            "invalid interval {s:?}: use a number with s, m or h, like 90s, 5m or 1h"
        ))
    };
    let t = s.trim().to_ascii_lowercase();
    let (digits, unit) = match t.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => t.split_at(i),
        None => (t.as_str(), "m"),
    };
    let n: u64 = digits.parse().map_err(|_| bad())?;
    let secs = match unit {
        "s" => n,
        "m" => n.checked_mul(60).ok_or_else(bad)?,
        "h" => n.checked_mul(3600).ok_or_else(bad)?,
        _ => return Err(bad()),
    };
    let d = Duration::from_secs(secs);
    if d < MIN_INTERVAL || d > MAX_INTERVAL {
        return Err(Error::BadInput(format!(
            "interval {s:?} is out of range: use between 1m and 24h"
        )));
    }
    Ok(d)
}

/// Where the plane probably is. Computed from the schedule, not received from the aircraft.
#[derive(Debug, Clone, Serialize)]
pub struct Estimate {
    /// Always true: this is a calculation, not live tracking.
    pub estimated: bool,
    pub latitude: f64,
    pub longitude: f64,
    /// Share of the flight time that has passed, 0 to 100.
    pub progress_pct: f64,
    pub flown_km: f64,
    pub remaining_km: f64,
    /// Average speed over the whole route for the expected flight time.
    pub average_speed_kmh: f64,
    /// Compass direction of travel at the estimated point, 0 to 360.
    pub heading_deg: f64,
    pub remaining_minutes: i64,
}

/// One reading of a flight.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    /// When the reading was taken (UTC).
    pub at: String,
    /// Flighty flight UUID; later readings follow this exact flight.
    pub flight_id: String,
    pub flight_code: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub phase: String,
    pub summary: String,
    pub departure: Option<Stamp>,
    pub arrival: Option<Stamp>,
    pub departure_delay_minutes: Option<i64>,
    pub arrival_delay_minutes: Option<i64>,
    pub departure_terminal: Option<String>,
    pub departure_gate: Option<String>,
    pub arrival_terminal: Option<String>,
    pub arrival_gate: Option<String>,
    pub baggage_belt: Option<String>,
    /// Only while the flight is in the air and both airports have coordinates.
    pub position: Option<Estimate>,
    /// What differs from the previous reading; empty on the first one.
    pub changes: Vec<String>,
    /// True once nothing more will change: landed, cancelled or diverted.
    pub done: bool,
}

/// Coordinates of the departure and (actual, else scheduled) arrival airport.
fn endpoints(conn: &Connection, flight_id: &str) -> Result<Option<[f64; 4]>> {
    let sql = "SELECT d.latitude, d.longitude, a.latitude, a.longitude FROM ( \
                 SELECT departureAirportId AS dep, \
                        COALESCE(actualArrivalAirportId, scheduledArrivalAirportId) AS arr \
                   FROM Flight WHERE id = ?1 \
                 UNION ALL \
                 SELECT departureAirportId, \
                        COALESCE(actualArrivalAirportId, scheduledArrivalAirportId) \
                   FROM ManualFlight WHERE id = ?1 \
               ) r JOIN Airport d ON d.id = r.dep JOIN Airport a ON a.id = r.arr LIMIT 1";
    let row: Option<[Option<f64>; 4]> = conn
        .query_row(sql, [flight_id], |r| {
            Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?])
        })
        .optional()?;
    Ok(row.and_then(|c| Some([c[0]?, c[1]?, c[2]?, c[3]?])))
}

/// Great-circle distance in km between two points given in degrees.
fn distance_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let (dp, dl) = (p2 - p1, (lon2 - lon1).to_radians());
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * a.sqrt().asin()
}

/// The point a fraction `f` of the way along the great circle, in degrees.
fn along(lat1: f64, lon1: f64, lat2: f64, lon2: f64, f: f64) -> (f64, f64) {
    let (p1, l1, p2, l2) = (
        lat1.to_radians(),
        lon1.to_radians(),
        lat2.to_radians(),
        lon2.to_radians(),
    );
    let d = distance_km(lat1, lon1, lat2, lon2) / EARTH_RADIUS_KM;
    if d < 1e-9 {
        return (lat1, lon1);
    }
    let (a, b) = (((1.0 - f) * d).sin() / d.sin(), (f * d).sin() / d.sin());
    let x = a * p1.cos() * l1.cos() + b * p2.cos() * l2.cos();
    let y = a * p1.cos() * l1.sin() + b * p2.cos() * l2.sin();
    let z = a * p1.sin() + b * p2.sin();
    (
        z.atan2((x * x + y * y).sqrt()).to_degrees(),
        y.atan2(x).to_degrees(),
    )
}

/// Compass bearing from the first point towards the second, 0 to 360.
fn bearing(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2, dl) = (
        lat1.to_radians(),
        lat2.to_radians(),
        (lon2 - lon1).to_radians(),
    );
    let y = dl.sin() * p2.cos();
    let x = p1.cos() * p2.sin() - p1.sin() * p2.cos() * dl.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Position from elapsed time between the best-known departure and arrival.
pub fn estimate(f: &Flight, [lat1, lon1, lat2, lon2]: [f64; 4], now: i64) -> Option<Estimate> {
    let start = f
        .takeoff_actual
        .as_ref()
        .map(|s| s.unix)
        .or_else(|| departure_time(f))?;
    let end = f
        .landing_actual
        .as_ref()
        .map(|s| s.unix)
        .or_else(|| arrival_time(f))?;
    if end <= start {
        return None;
    }
    let frac = ((now - start) as f64 / (end - start) as f64).clamp(0.0, 1.0);
    let total = distance_km(lat1, lon1, lat2, lon2);
    let (lat, lon) = along(lat1, lon1, lat2, lon2, frac);
    Some(Estimate {
        estimated: true,
        latitude: (lat * 1000.0).round() / 1000.0,
        longitude: (lon * 1000.0).round() / 1000.0,
        progress_pct: round1(frac * 100.0),
        flown_km: (total * frac).round(),
        remaining_km: (total * (1.0 - frac)).round(),
        average_speed_kmh: (total / ((end - start) as f64 / 3600.0)).round(),
        heading_deg: bearing(lat, lon, lat2, lon2).round() % 360.0,
        remaining_minutes: ((end - now).max(0) + 30) / 60,
    })
}

fn best(stamps: [&Option<Stamp>; 3]) -> Option<Stamp> {
    stamps.into_iter().find_map(|s| s.clone())
}

fn build(st: FlightStatus, coords: Option<[f64; 4]>, now: i64) -> Snapshot {
    let f = &st.flight;
    let position = (st.phase == "in_air")
        .then(|| coords.and_then(|c| estimate(f, c, now)))
        .flatten();
    Snapshot {
        at: Stamp::new(now, None).map(|s| s.utc).unwrap_or_default(),
        flight_id: f.id.clone(),
        flight_code: f.flight_code.clone(),
        from: f.from.iata.clone(),
        to: f.diverted_to.as_ref().unwrap_or(&f.to).iata.clone(),
        done: matches!(st.phase.as_str(), "landed" | "cancelled" | "diverted"),
        departure: best([
            &f.departure_actual,
            &f.departure_estimated,
            &f.departure_scheduled,
        ]),
        arrival: best([
            &f.arrival_actual,
            &f.arrival_estimated,
            &f.arrival_scheduled,
        ]),
        departure_delay_minutes: st.departure_delay_minutes,
        arrival_delay_minutes: st.arrival_delay_minutes,
        departure_terminal: f.departure_terminal.clone(),
        departure_gate: f.departure_gate.clone(),
        arrival_terminal: f.arrival_terminal.clone(),
        arrival_gate: f.arrival_gate.clone(),
        baggage_belt: f.baggage_belt.clone(),
        position,
        changes: Vec::new(),
        phase: st.phase,
        summary: st.summary,
    }
}

/// Reads the flight now. Pass the previous reading to get `changes` filled in.
pub fn snapshot(ctx: &Ctx, a: &FlightRef, previous: Option<&Snapshot>) -> Result<Snapshot> {
    snapshot_at(ctx, a, previous, time::now_unix())
}

/// `snapshot` at a given time (unix seconds).
#[doc(hidden)]
pub fn snapshot_at(
    ctx: &Ctx,
    a: &FlightRef,
    previous: Option<&Snapshot>,
    now: i64,
) -> Result<Snapshot> {
    let (conn, flight) = status::resolve(ctx, a)?;
    let coords = endpoints(&conn, &flight.id)?;
    drop(conn);
    let mut snap = build(status::status_at(flight, now), coords, now);
    if let Some(prev) = previous {
        snap.changes = changes(prev, &snap);
    }
    Ok(snap)
}

/// Human-readable differences between two readings, e.g. "gate A1 → B4".
pub fn changes(old: &Snapshot, new: &Snapshot) -> Vec<String> {
    let text = |v: &Option<String>| v.clone().unwrap_or_else(|| "—".into());
    let when = |v: &Option<Stamp>| v.as_ref().map(Stamp::human).unwrap_or_else(|| "—".into());
    let mut out = Vec::new();
    if old.phase != new.phase {
        out.push(format!("status {} → {}", old.phase, new.phase));
    }
    let unix = |v: &Option<Stamp>| v.as_ref().map(|s| s.unix);
    for (label, a, b) in [
        ("departure", &old.departure, &new.departure),
        ("arrival", &old.arrival, &new.arrival),
    ] {
        if unix(a) != unix(b) {
            out.push(format!("{label} {} → {}", when(a), when(b)));
        }
    }
    for (label, a, b) in [
        (
            "departure terminal",
            &old.departure_terminal,
            &new.departure_terminal,
        ),
        ("departure gate", &old.departure_gate, &new.departure_gate),
        (
            "arrival terminal",
            &old.arrival_terminal,
            &new.arrival_terminal,
        ),
        ("arrival gate", &old.arrival_gate, &new.arrival_gate),
        ("baggage belt", &old.baggage_belt, &new.baggage_belt),
    ] {
        if a != b {
            out.push(format!("{label} {} → {}", text(a), text(b)));
        }
    }
    out
}

fn coordinate(v: f64, positive: char, negative: char) -> String {
    format!(
        "{:.2}°{}",
        v.abs(),
        if v < 0.0 { negative } else { positive }
    )
}

impl Snapshot {
    /// The status sentence, plus the estimated position while in the air.
    pub fn line(&self) -> String {
        match &self.position {
            Some(p) => format!(
                "{} · about {:.0}% of the way, {:.0} km to go, {} left · near {} {}, heading {:.0}°, averaging {:.0} km/h (estimated from the schedule, not live tracking)",
                self.summary,
                p.progress_pct,
                p.remaining_km,
                time::duration(p.remaining_minutes),
                coordinate(p.latitude, 'N', 'S'),
                coordinate(p.longitude, 'E', 'W'),
                p.heading_deg,
                p.average_speed_kmh
            ),
            None => self.summary.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_intervals() {
        assert_eq!(parse_interval("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_interval(" 90S ").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_interval("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_interval("15").unwrap(), Duration::from_secs(900));
        assert_eq!(parse_interval("24h").unwrap(), MAX_INTERVAL);
        for bad in ["", "m", "5x", "30s", "0", "25h", "-5m", "1.5h", "5 m"] {
            assert!(
                matches!(parse_interval(bad), Err(Error::BadInput(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn great_circle_maths() {
        // London Heathrow to New York JFK is about 5,540 km.
        let (lhr, jfk) = ((51.4706, -0.4619), (40.6413, -73.7781));
        let d = distance_km(lhr.0, lhr.1, jfk.0, jfk.1);
        assert!((5500.0..5600.0).contains(&d), "{d}");
        // It starts out west-north-west and the halfway point is further north than both ends.
        let b = bearing(lhr.0, lhr.1, jfk.0, jfk.1);
        assert!((280.0..295.0).contains(&b), "{b}");
        let (lat, lon) = along(lhr.0, lhr.1, jfk.0, jfk.1, 0.5);
        assert!(lat > 51.5 && (-45.0..-35.0).contains(&lon), "{lat} {lon}");
        for (f, want) in [(0.0, lhr), (1.0, jfk)] {
            let got = along(lhr.0, lhr.1, jfk.0, jfk.1, f);
            assert!(
                (got.0 - want.0).abs() < 1e-6 && (got.1 - want.1).abs() < 1e-6,
                "{got:?}"
            );
        }
    }
}
