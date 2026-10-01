//! Live-ish status of one flight from the local DB, and a delay forecast from its history.

use rusqlite::{Connection, OptionalExtension};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;
use crate::db::{self, col_or_null};
use crate::error::Result;
use crate::model::Flight;
use crate::ops::flights::{self, arrival_delay_seconds, arrival_time, departure_delay_seconds};
use crate::owner;
use crate::render::{self, Render};
use crate::time::{self, Stamp};

#[derive(Debug, Clone, Deserialize, JsonSchema, clap::Args)]
pub struct FlightRef {
    /// Flight code ("BA286") or Flighty flight UUID.
    pub flight: String,
    /// Departure date YYYY-MM-DD (local). Default: nearest to now.
    #[arg(long)]
    pub date: Option<String>,
}

/// Status summary: phase (scheduled/departed/in_air/landed/cancelled/diverted),
/// delays, gates, terminals, belt, times.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FlightStatus {
    pub flight: crate::model::Flight,
    pub phase: String,
    pub departure_delay_minutes: Option<i64>,
    pub arrival_delay_minutes: Option<i64>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DelayForecast {
    pub flight_code: String,
    /// Past flights of the same code found locally.
    pub samples: usize,
    pub on_time_pct: Option<f64>,
    pub avg_delay_minutes: Option<f64>,
    pub max_delay_minutes: Option<i64>,
    /// Plain-language note, e.g. "not enough history".
    pub note: String,
    /// "flighty" (the app's own forecast for this flight) | "local_history" | "none".
    pub source: String,
}

/// Arrivals less than this late count as on time (the usual 15-minute rule).
const ON_TIME_MINUTES: i64 = 15;
/// Fewer completed flights than this is "not enough history".
const MIN_SAMPLES: usize = 2;

impl Render for FlightStatus {
    fn table(&self) -> String {
        format!("{}\n\n{}", self.summary, self.flight.table())
    }
}

impl Render for DelayForecast {
    fn table(&self) -> String {
        let pct = self.on_time_pct.map(|p| format!("{p:.0}%"));
        let avg = self.avg_delay_minutes.map(|m| format!("{m:.0} min"));
        let max = self.max_delay_minutes.map(|m| format!("{m} min"));
        [
            ("Flight", Some(self.flight_code.clone())),
            ("Samples", Some(self.samples.to_string())),
            ("On time", Some(render::dash(pct))),
            ("Avg delay", Some(render::dash(avg))),
            ("Max delay", max),
            ("Source", Some(self.source.clone())),
            ("Note", Some(self.note.clone())),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.map(|v| format!("{k:<9}  {v}")))
        .collect::<Vec<_>>()
        .join("\n")
    }
}

/// Phase from the recorded times. Flights with no live data whose arrival has passed are
/// assumed landed (manual and old flights never get actual times).
fn phase(f: &Flight, now: i64) -> &'static str {
    let passed = |s: &Option<Stamp>| s.as_ref().is_some_and(|s| s.unix <= now);
    if f.cancelled {
        "cancelled"
    } else if f.diverted {
        "diverted"
    } else if passed(&f.arrival_actual) || passed(&f.landing_actual) {
        "landed"
    } else if passed(&f.takeoff_actual) {
        "in_air"
    } else if passed(&f.departure_actual) {
        // Off the gate: airborne until the expected arrival; after that we only know it left.
        if arrival_time(f).is_some_and(|a| a > now) {
            "in_air"
        } else {
            "departed"
        }
    } else if arrival_time(f).is_some_and(|a| a <= now) {
        "landed"
    } else {
        "scheduled"
    }
}

fn summary(f: &Flight, phase: &str, now: i64) -> String {
    let head = format!(
        "{} {} → {}",
        f.flight_code,
        f.from.iata.as_deref().unwrap_or("?"),
        f.to.iata.as_deref().unwrap_or("?")
    );
    let when = |s: Option<&Stamp>| render::dash(s.map(Stamp::human));
    let dep = f
        .departure_actual
        .as_ref()
        .or(f.departure_estimated.as_ref())
        .or(f.departure_scheduled.as_ref());
    let arr = f
        .arrival_actual
        .as_ref()
        .or(f.arrival_estimated.as_ref())
        .or(f.arrival_scheduled.as_ref());
    let late = |d: Option<i64>| {
        d.map(|d| format!(" ({})", time::delay(d)))
            .unwrap_or_default()
    };
    let dep_delay = late(departure_delay_seconds(f));
    let arr_delay = late(arrival_delay_seconds(f));
    let gate = f
        .departure_gate
        .as_deref()
        .map(|g| format!(", gate {g}"))
        .unwrap_or_default();
    match phase {
        "cancelled" => format!(
            "{head} is CANCELLED (was due {})",
            when(f.departure_scheduled.as_ref())
        ),
        "diverted" => format!(
            "{head} was DIVERTED to {}",
            f.diverted_to
                .as_ref()
                .and_then(|a| a.iata.as_deref())
                .unwrap_or("another airport")
        ),
        "landed" if f.arrival_actual.is_none() && f.landing_actual.is_none() => {
            format!("{head} arrived {} (scheduled; no live data)", when(arr))
        }
        "landed" => format!("{head} landed {}{arr_delay}", when(arr)),
        "in_air" => format!(
            "{head} in the air: departed {}{dep_delay}, arriving {}{arr_delay}",
            when(dep),
            when(arr)
        ),
        "departed" => format!(
            "{head} departed {}{dep_delay}; arrival not reported yet",
            when(dep)
        ),
        _ => {
            let mins = dep.map(|d| (d.unix - now) / 60).filter(|m| *m > 0);
            let in_ = mins
                .map(|m| format!(" (in {})", time::duration(m)))
                .unwrap_or_default();
            format!("{head} departs {}{in_}{dep_delay}{gate}", when(dep))
        }
    }
}

fn resolve(ctx: &Ctx, a: &FlightRef) -> Result<(Connection, Flight)> {
    let conn = db::open(ctx)?;
    let owner = owner::resolve(ctx, &conn)?;
    let flight = flights::find(&conn, &owner.user_id, &a.flight, a.date.as_deref())?;
    Ok((conn, flight))
}

pub fn flight_status(ctx: &Ctx, a: &FlightRef) -> Result<FlightStatus> {
    let (conn, flight) = resolve(ctx, a)?;
    drop(conn);
    let now = time::now_unix();
    let phase = phase(&flight, now);
    Ok(FlightStatus {
        phase: phase.to_string(),
        departure_delay_minutes: departure_delay_seconds(&flight).map(|s| s / 60),
        arrival_delay_minutes: arrival_delay_seconds(&flight).map(|s| s / 60),
        summary: summary(&flight, phase, now),
        flight,
    })
}

/// Flighty's own forecast columns for this flight (counts over recent operations).
struct AppForecast {
    observations: i64,
    mean: Option<i64>,
    early: i64,
    on_time: i64,
    late15: i64,
    late30: i64,
    late45: i64,
    canceled: i64,
}

fn app_forecast(conn: &Connection, flight: &Flight) -> Result<Option<AppForecast>> {
    let table = if flight.manual {
        "ManualFlight"
    } else {
        "Flight"
    };
    let c = |name: &str| col_or_null(conn, table, "f", &[name]);
    let sql = format!(
        "SELECT {}, {}, {}, {}, {}, {}, {}, {} FROM {table} f WHERE f.id = ?1",
        c("delayForecastObservations")?,
        c("delayForecastDelayMean")?,
        c("delayForecastEarlyCount")?,
        c("delayForecastOntimeCount")?,
        c("delayForecastLate15Count")?,
        c("delayForecastLate30Count")?,
        c("delayForecastLate45Count")?,
        c("delayForecastCanceledCount")?,
    );
    let row = conn
        .query_row(&sql, [&flight.id], |r| {
            let n = |i: usize| r.get::<_, Option<i64>>(i).map(Option::unwrap_or_default);
            Ok(AppForecast {
                observations: n(0)?,
                mean: r.get(1)?,
                early: n(2)?,
                on_time: n(3)?,
                late15: n(4)?,
                late30: n(5)?,
                late45: n(6)?,
                canceled: n(7)?,
            })
        })
        .optional()?;
    Ok(row.filter(|f| f.observations > 0))
}

/// Arrival delays (minutes) of completed flights with this code in the local DB, any user.
fn local_history(conn: &Connection, flight: &Flight) -> Result<Vec<i64>> {
    let airline = flight
        .flight_code
        .strip_suffix(flight.number.as_str())
        .unwrap_or("");
    let sql = format!(
        "SELECT f.arrivalScheduleGateOriginal, {actual} FROM Flight f \
         LEFT JOIN Airline al ON al.id = f.airlineId \
         WHERE {deleted} IS NULL AND f.isCancelled = 0 \
           AND ltrim(f.number, '0') = ltrim(?1, '0') \
           AND upper(COALESCE(al.iata, {icao}, '')) = ?2",
        actual = col_or_null(conn, "Flight", "f", &["arrivalScheduleGateActual"])?,
        deleted = col_or_null(conn, "Flight", "f", &["deleted"])?,
        icao = col_or_null(conn, "Airline", "al", &["icao"])?,
    );
    let now = time::now_unix();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([flight.number.as_str(), airline], |r| {
        Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?))
    })?;
    let mut delays = Vec::new();
    for row in rows {
        if let (Some(sched), Some(actual)) = row?
            && sched > 0
            && actual > 0
            && actual <= now
        {
            delays.push((actual - sched) / 60);
        }
    }
    Ok(delays)
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

pub fn delay_forecast(ctx: &Ctx, a: &FlightRef) -> Result<DelayForecast> {
    let (conn, flight) = resolve(ctx, a)?;
    let code = flight.flight_code.clone();

    if let Some(fc) = app_forecast(&conn, &flight)? {
        let obs = fc.observations as f64;
        let pct = |n: i64| round1(n as f64 * 100.0 / obs);
        let on_time = pct(fc.early + fc.on_time);
        return Ok(DelayForecast {
            note: format!(
                "Flighty's forecast from {} recent flights: {on_time:.0}% on time, {:.0}% 15–29 min late, \
                 {:.0}% 30–44 min late, {:.0}% 45+ min late, {:.0}% cancelled",
                fc.observations,
                pct(fc.late15),
                pct(fc.late30),
                pct(fc.late45),
                pct(fc.canceled),
            ),
            flight_code: code,
            samples: fc.observations as usize,
            on_time_pct: Some(on_time),
            avg_delay_minutes: fc.mean.map(|m| m as f64),
            max_delay_minutes: None,
            source: "flighty".into(),
        });
    }

    let delays = local_history(&conn, &flight)?;
    let n = delays.len();
    if n < MIN_SAMPLES {
        return Ok(DelayForecast {
            note: format!(
                "not enough history: {n} completed {code} flight(s) in the local database and no Flighty forecast"
            ),
            flight_code: code,
            samples: n,
            on_time_pct: None,
            avg_delay_minutes: None,
            max_delay_minutes: None,
            source: "none".into(),
        });
    }
    let on_time = delays.iter().filter(|d| **d < ON_TIME_MINUTES).count();
    let pct = round1(on_time as f64 * 100.0 / n as f64);
    Ok(DelayForecast {
        note: format!(
            "from {n} past {code} flights in the local database (arrival delay; on time = under {ON_TIME_MINUTES} min late){}",
            if n < 5 {
                "; few samples, treat as rough"
            } else {
                ""
            }
        ),
        flight_code: code,
        samples: n,
        on_time_pct: Some(pct),
        avg_delay_minutes: Some(round1(delays.iter().sum::<i64>() as f64 / n as f64)),
        max_delay_minutes: delays.iter().copied().max(),
        source: "local_history".into(),
    })
}
