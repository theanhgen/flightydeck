//! `ics`: the owner's flights as an iCalendar file (RFC 5545). Local database only.

use chrono::{TimeZone, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;
use crate::error::Result;
use crate::model::{AirportRef, Flight, Relation};
use crate::ops::flights::{self, GetArgs, ListArgs};
use crate::render::Render;
use crate::time::{self, Stamp};

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, clap::Args)]
pub struct IcsArgs {
    /// Only this flight: code ("BA286") or Flighty UUID. Default: all your flights.
    pub flight: Option<String>,
    /// With a flight code: departure date YYYY-MM-DD (local). Default: nearest to now.
    #[arg(long, requires = "flight")]
    pub date: Option<String>,
    /// Only flights departing from now on.
    #[arg(long)]
    #[serde(default)]
    pub upcoming: bool,
    /// Only flights that already departed.
    #[arg(long, conflicts_with = "upcoming")]
    #[serde(default)]
    pub past: bool,
    /// Only flights departing in this calendar year (local date at the departure airport).
    #[arg(long)]
    pub year: Option<i32>,
    /// Include flights archived in the app.
    #[arg(long)]
    #[serde(default)]
    pub include_archived: bool,
    /// Include flights the owner follows but doesn't fly.
    #[arg(long)]
    #[serde(default)]
    pub include_following: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Calendar {
    /// Number of events in the calendar.
    pub count: usize,
    /// Flights left out because they have no departure time.
    pub skipped: usize,
    /// The iCalendar text (CRLF line endings).
    pub ics: String,
}

impl Render for Calendar {
    fn table(&self) -> String {
        self.ics.clone()
    }
}

pub fn ics(ctx: &Ctx, a: &IcsArgs) -> Result<Calendar> {
    let flights = match &a.flight {
        Some(flight) => vec![flights::get(
            ctx,
            &GetArgs {
                flight: flight.clone(),
                date: a.date.clone(),
            },
        )?],
        None => {
            flights::list(
                ctx,
                &ListArgs {
                    upcoming: a.upcoming,
                    past: a.past,
                    include_archived: a.include_archived,
                    include_following: a.include_following,
                    year: a.year,
                    limit: Some(u32::MAX),
                    offset: None,
                },
            )?
            .flights
        }
    };
    Ok(calendar(&flights, time::now_unix()))
}

/// The calendar for `flights`, stamped `now` (unix seconds).
pub fn calendar(flights: &[Flight], now: i64) -> Calendar {
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".into(),
        "PRODID:-//flightydeck//EN".into(),
        "CALSCALE:GREGORIAN".into(),
        "X-WR-CALNAME:Flights".into(),
    ];
    let mut count = 0;
    for f in flights {
        if let Some(event) = event(f, now) {
            lines.extend(event);
            count += 1;
        }
    }
    lines.push("END:VCALENDAR".into());
    let ics = lines.iter().map(|l| fold(l) + "\r\n").collect();
    Calendar {
        count,
        skipped: flights.len() - count,
        ics,
    }
}

/// One `VEVENT`, unfolded. `None` when the flight has no departure time.
fn event(f: &Flight, now: i64) -> Option<Vec<String>> {
    let start = flights::departure_time(f)?;
    let end = flights::arrival_time(f)
        .or_else(|| f.duration_minutes.map(|m| start + m * 60))
        .filter(|end| *end > start);

    let mut lines = vec![
        "BEGIN:VEVENT".to_string(),
        // The flight's UUID, so importing again updates the event instead of adding a second one.
        format!("UID:{}@flightydeck", f.id),
        format!("DTSTAMP:{}", utc(now)?),
        format!("DTSTART:{}", utc(start)?),
    ];
    if let Some(end) = end.and_then(utc) {
        lines.push(format!("DTEND:{end}"));
    }
    lines.push(format!("SUMMARY:{}", escape(&summary(f))));
    if let Some(location) = location(f) {
        lines.push(format!("LOCATION:{}", escape(&location)));
    }
    lines.push(format!("DESCRIPTION:{}", escape(&description(f))));
    if f.cancelled {
        lines.push("STATUS:CANCELLED".into());
    }
    lines.push("END:VEVENT".into());
    Some(lines)
}

/// "20310317T145000Z"
fn utc(unix: i64) -> Option<String> {
    let dt = Utc.timestamp_opt(unix, 0).single()?;
    Some(dt.format("%Y%m%dT%H%M%SZ").to_string())
}

fn code(a: &AirportRef) -> String {
    a.iata
        .clone()
        .or_else(|| a.icao.clone())
        .or_else(|| a.name.clone())
        .unwrap_or_else(|| "?".into())
}

/// "BA286 SFO → LHR"
fn summary(f: &Flight) -> String {
    let to = f.diverted_to.as_ref().unwrap_or(&f.to);
    let s = format!("{} {} → {}", f.flight_code, code(&f.from), code(to));
    if f.cancelled {
        format!("Cancelled: {s}")
    } else {
        s
    }
}

/// "London Heathrow (LHR), Terminal 1"
fn location(f: &Flight) -> Option<String> {
    let a = &f.from;
    let mut s = match (&a.name, &a.iata) {
        (Some(name), Some(iata)) => format!("{name} ({iata})"),
        (Some(one), None) | (None, Some(one)) => one.clone(),
        (None, None) => a.icao.clone()?,
    };
    if let Some(t) = &f.departure_terminal {
        s.push_str(&format!(", Terminal {t}"));
    }
    Some(s)
}

fn description(f: &Flight) -> String {
    let best = |stamps: [&Option<Stamp>; 3]| {
        stamps
            .into_iter()
            .find_map(|s| s.as_ref())
            .map(Stamp::human)
    };
    let place = |a: &AirportRef| match (&a.city, &a.name) {
        (Some(city), _) => format!("{city} ({})", code(a)),
        (None, Some(name)) => format!("{name} ({})", code(a)),
        (None, None) => code(a),
    };
    let join = |parts: [(&str, &Option<String>); 2]| {
        let v: Vec<String> = parts
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k} {v}")))
            .collect();
        (!v.is_empty()).then(|| v.join(", "))
    };

    let mut out = vec![match &f.airline_name {
        Some(name) => format!("{name} {}", f.flight_code),
        None => f.flight_code.clone(),
    }];
    let mut leg = |label: &str, a: &AirportRef, when: Option<String>, at: Option<String>| {
        let mut s = format!("{label}: {}", place(a));
        for part in [when, at].into_iter().flatten() {
            s.push_str(&format!(", {part}"));
        }
        out.push(s);
    };
    leg(
        "Departs",
        &f.from,
        best([
            &f.departure_actual,
            &f.departure_estimated,
            &f.departure_scheduled,
        ]),
        join([
            ("terminal", &f.departure_terminal),
            ("gate", &f.departure_gate),
        ]),
    );
    leg(
        "Arrives",
        f.diverted_to.as_ref().unwrap_or(&f.to),
        best([
            &f.arrival_actual,
            &f.arrival_estimated,
            &f.arrival_scheduled,
        ]),
        join([("terminal", &f.arrival_terminal), ("gate", &f.arrival_gate)]),
    );
    if f.diverted {
        out.push(format!("Diverted; was going to {}", place(&f.to)));
    }
    if let Some(t) = &f.ticket {
        for (label, v) in [
            ("Seat", &t.seat),
            ("Cabin", &t.cabin),
            ("Booking", &t.booking_reference),
        ] {
            if let Some(v) = v {
                out.push(format!("{label}: {v}"));
            }
        }
    }
    match (&f.aircraft, &f.tail_number) {
        (Some(a), Some(t)) => out.push(format!("Aircraft: {a} ({t})")),
        (Some(one), None) | (None, Some(one)) => out.push(format!("Aircraft: {one}")),
        (None, None) => {}
    }
    match f.relation {
        Relation::Mine => {}
        Relation::Following => out.push("Following (not on this flight)".into()),
        Relation::Friend => out.push(format!(
            "Friend: {}",
            f.friend_name.as_deref().unwrap_or("?")
        )),
    }
    out.join("\n")
}

/// RFC 5545 TEXT escaping: backslash, semicolon, comma, newline.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// Folds a content line to at most 75 octets per physical line, never inside a character.
/// Continuation lines start with one space, which counts towards the 75.
fn fold(line: &str) -> String {
    const MAX: usize = 75;
    let mut out = String::with_capacity(line.len() + line.len() / MAX * 3);
    let mut used = 0;
    for c in line.chars() {
        if used + c.len_utf8() > MAX {
            out.push_str("\r\n ");
            used = 1;
        }
        out.push(c);
        used += c.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_text() {
        assert_eq!(escape("a,b;c\\d\r\ne"), "a\\,b\\;c\\\\d\\ne");
    }

    #[test]
    fn folds_at_75_octets_without_splitting_characters() {
        assert_eq!(fold("SUMMARY:short"), "SUMMARY:short");
        let exact = "x".repeat(75);
        assert_eq!(fold(&exact), exact);

        // 74 ASCII octets, then two-octet characters: the first one must move to the next line.
        let line = format!("{}{}", "x".repeat(74), "é".repeat(40));
        let folded = fold(&line);
        let physical: Vec<&str> = folded.split("\r\n").collect();
        assert!(physical.len() > 1);
        assert!(physical.iter().all(|l| l.len() <= 75), "{physical:?}");
        assert!(physical[1..].iter().all(|l| l.starts_with(' ')));
        let unfolded: String = folded.replace("\r\n ", "");
        assert_eq!(unfolded, line);
    }
}
