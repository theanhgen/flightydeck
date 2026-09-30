//! Lifetime / per-year flight statistics for the owner (own flights only, not followed).
//!
//! Counts every non-cancelled flight the owner flies, past and upcoming, archived or not.
//! `Flight.distance` is already great-circle kilometres (checked against Airport lat/long).

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;
use crate::db;
use crate::error::Result;
use crate::render::{self, Render};
use crate::time;

const TOP_N: usize = 5;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, clap::Args)]
pub struct StatsArgs {
    /// Only this calendar year.
    #[arg(long)]
    pub year: Option<i32>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Stats {
    pub year: Option<i32>,
    pub flights: usize,
    pub distance_km: f64,
    pub air_minutes: i64,
    pub airports: usize,
    /// Each country counted once.
    pub countries: usize,
    pub airlines: usize,
    pub aircraft_types: usize,
    pub top_airlines: Vec<(String, usize)>,
    pub top_airports: Vec<(String, usize)>,
    pub top_routes: Vec<(String, usize)>,
    pub longest: Option<String>,
}

impl Render for Stats {
    fn table(&self) -> String {
        let scope = self.year.map_or("All time".to_string(), |y| y.to_string());
        let summary = vec![
            vec!["Flights".into(), self.flights.to_string()],
            vec!["Distance".into(), format!("{:.0} km", self.distance_km)],
            vec!["Time in air".into(), time::duration(self.air_minutes)],
            vec!["Airports".into(), self.airports.to_string()],
            vec!["Countries".into(), self.countries.to_string()],
            vec!["Airlines".into(), self.airlines.to_string()],
            vec!["Aircraft types".into(), self.aircraft_types.to_string()],
            vec!["Longest".into(), render::dash(self.longest.as_ref())],
        ];
        let top = |title: &str, items: &[(String, usize)]| {
            let rows: Vec<Vec<String>> = items
                .iter()
                .map(|(k, n)| vec![k.clone(), n.to_string()])
                .collect();
            render::table(&[title, "Flights"], &rows)
        };
        [
            render::table(&[&scope, ""], &summary),
            top("Top airlines", &self.top_airlines),
            top("Top airports", &self.top_airports),
            top("Top routes", &self.top_routes),
        ]
        .join("\n\n")
    }
}

/// One of the owner's flights, reduced to what the statistics need.
struct Leg {
    code: String,
    airline: Option<String>,
    dep: Option<String>,
    arr: Option<String>,
    dep_country: Option<String>,
    arr_country: Option<String>,
    dep_tz: Option<String>,
    aircraft: Option<String>,
    distance_km: i64,
    dep_time: Option<i64>,
    air_minutes: Option<i64>,
}

/// "QR", or ICAO, or nothing; then the number.
fn flight_code(iata: Option<String>, icao: Option<String>, number: Option<String>) -> String {
    let prefix = iata.or(icao).unwrap_or_default();
    format!("{prefix}{}", number.unwrap_or_default())
}

// Both halves select the same columns. ManualFlight has no *Estimated columns.
const LEGS_SQL: &str = r#"
SELECT f.number, al.iata, al.icao, al.name,
       coalesce(d.iata, d.icao, d.name), coalesce(a.iata, a.icao, a.name),
       d.countryCode, a.countryCode, d.timeZoneIdentifier,
       nullif(f.equipmentModelName, ''), f.distance,
       f.departureScheduleGateOriginal, f.departureScheduleGateEstimated, f.departureScheduleGateActual,
       f.arrivalScheduleGateOriginal, f.arrivalScheduleGateEstimated, f.arrivalScheduleGateActual,
       NULL
FROM UserFlight uf
JOIN Flight f ON f.id = uf.flightId
LEFT JOIN Airline al ON al.id = f.airlineId
LEFT JOIN Airport d ON d.id = f.departureAirportId
LEFT JOIN Airport a ON a.id = coalesce(nullif(f.actualArrivalAirportId, ''), f.scheduledArrivalAirportId)
WHERE uf.userId = ?1 AND uf.isMyFlight = 1 AND uf.deleted IS NULL
  AND f.deleted IS NULL AND f.isCancelled = 0
UNION ALL
SELECT f.number, al.iata, al.icao, al.name,
       coalesce(d.iata, d.icao, d.name), coalesce(a.iata, a.icao, a.name),
       d.countryCode, a.countryCode, d.timeZoneIdentifier,
       nullif(f.equipmentModelName, ''), f.distance,
       f.departureScheduleGateOriginal, NULL, f.departureScheduleGateActual,
       f.arrivalScheduleGateOriginal, NULL, f.arrivalScheduleGateActual,
       f.durationMinutes
FROM UserManualFlight uf
JOIN ManualFlight f ON f.id = uf.flightId
LEFT JOIN Airline al ON al.id = f.airlineId
LEFT JOIN Airport d ON d.id = f.departureAirportId
LEFT JOIN Airport a ON a.id = coalesce(nullif(f.actualArrivalAirportId, ''), f.scheduledArrivalAirportId)
WHERE uf.userId = ?1 AND uf.isMyFlight = 1 AND uf.deleted IS NULL
  AND f.deleted IS NULL AND f.isCancelled = 0
"#;

/// Best-known time: actual, then estimated, then scheduled (0/NULL = unknown).
fn best(actual: Option<i64>, estimated: Option<i64>, scheduled: Option<i64>) -> Option<i64> {
    [actual, estimated, scheduled]
        .into_iter()
        .flatten()
        .find(|t| *t > 0)
}

fn legs(conn: &Connection, owner_id: &str) -> Result<Vec<Leg>> {
    let mut stmt = conn.prepare(LEGS_SQL)?;
    let rows = stmt.query_map([owner_id], |r| {
        let dep_time = best(r.get(13)?, r.get(12)?, r.get(11)?);
        let arr_time = best(r.get(16)?, r.get(15)?, r.get(14)?);
        let manual_minutes: Option<i64> = r.get(17)?;
        let air_minutes = match (dep_time, arr_time) {
            (Some(d), Some(a)) if a > d => Some((a - d) / 60),
            _ => manual_minutes.filter(|m| *m > 0),
        };
        let al_iata: Option<String> = r.get(1)?;
        let al_icao: Option<String> = r.get(2)?;
        let al_name: Option<String> = r.get(3)?;
        Ok(Leg {
            code: flight_code(al_iata.clone(), al_icao.clone(), r.get(0)?),
            airline: al_name.or(al_iata).or(al_icao),
            dep: r.get(4)?,
            arr: r.get(5)?,
            dep_country: r.get(6)?,
            arr_country: r.get(7)?,
            dep_tz: r.get(8)?,
            aircraft: r.get(9)?,
            distance_km: r.get::<_, Option<i64>>(10)?.unwrap_or(0),
            // The year filter uses the scheduled departure, falling back to the best-known one.
            dep_time: r.get::<_, Option<i64>>(11)?.filter(|t| *t > 0).or(dep_time),
            air_minutes,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Highest counts first, ties alphabetical, at most `TOP_N`.
fn top(counts: HashMap<String, usize>) -> Vec<(String, usize)> {
    let mut v: Vec<_> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(TOP_N);
    v
}

fn year_of(leg: &Leg) -> Option<i32> {
    let date = time::local_date(leg.dep_time?, leg.dep_tz.as_deref())?;
    date.get(..4)?.parse().ok()
}

/// Statistics for a known owner id (`stats` resolves the owner first).
#[doc(hidden)]
pub fn stats_for(conn: &Connection, owner_id: &str, a: &StatsArgs) -> Result<Stats> {
    let legs: Vec<Leg> = legs(conn, owner_id)?
        .into_iter()
        .filter(|l| a.year.is_none() || year_of(l) == a.year)
        .collect();

    let mut airports = HashSet::new();
    let mut countries = HashSet::new();
    let mut aircraft = HashSet::new();
    let mut by_airline: HashMap<String, usize> = HashMap::new();
    let mut by_airport: HashMap<String, usize> = HashMap::new();
    let mut by_route: HashMap<String, usize> = HashMap::new();
    for l in &legs {
        for ap in [&l.dep, &l.arr].into_iter().flatten() {
            airports.insert(ap.clone());
            *by_airport.entry(ap.clone()).or_default() += 1;
        }
        // One set over departure and arrival countries: a round trip is 2 countries, not 4.
        countries.extend(
            [&l.dep_country, &l.arr_country]
                .into_iter()
                .flatten()
                .cloned(),
        );
        if let Some(t) = &l.aircraft {
            aircraft.insert(t.clone());
        }
        if let Some(al) = &l.airline {
            *by_airline.entry(al.clone()).or_default() += 1;
        }
        if let (Some(d), Some(r)) = (&l.dep, &l.arr) {
            *by_route.entry(format!("{d}–{r}")).or_default() += 1;
        }
    }

    let longest = legs.iter().max_by_key(|l| l.distance_km).map(|l| {
        let date = l
            .dep_time
            .and_then(|t| time::local_date(t, l.dep_tz.as_deref()))
            .unwrap_or_default();
        format!(
            "{} {}–{}, {} km, {date}",
            l.code,
            l.dep.as_deref().unwrap_or("?"),
            l.arr.as_deref().unwrap_or("?"),
            l.distance_km
        )
        .trim_end_matches(", ")
        .to_string()
    });

    Ok(Stats {
        year: a.year,
        flights: legs.len(),
        distance_km: legs.iter().map(|l| l.distance_km as f64).sum(),
        air_minutes: legs.iter().filter_map(|l| l.air_minutes).sum(),
        airports: airports.len(),
        countries: countries.len(),
        airlines: by_airline.len(),
        aircraft_types: aircraft.len(),
        top_airlines: top(by_airline),
        top_airports: top(by_airport),
        top_routes: top(by_route),
        longest,
    })
}

pub fn stats(ctx: &Ctx, a: &StatsArgs) -> Result<Stats> {
    let conn = db::open(ctx)?;
    let owner = crate::owner::resolve(ctx, &conn)?;
    stats_for(&conn, &owner.user_id, a)
}
