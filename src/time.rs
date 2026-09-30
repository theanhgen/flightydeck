//! Timestamps: DB stores unix seconds (UTC). Output carries UTC and airport-local time.

use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::Serialize;

/// `{ "utc": "2026-10-17T14:50:00Z", "local": "2026-10-17 16:50", "tz": "Europe/Prague" }`
#[derive(Debug, Clone, Serialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct Stamp {
    pub utc: String,
    pub local: Option<String>,
    pub tz: Option<String>,
    #[serde(skip)]
    pub unix: i64,
}

impl Stamp {
    pub fn new(unix: i64, tz: Option<&str>) -> Option<Stamp> {
        let dt: DateTime<Utc> = Utc.timestamp_opt(unix, 0).single()?;
        let parsed: Option<Tz> = tz.and_then(|t| t.parse().ok());
        Some(Stamp {
            utc: dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            local: parsed.map(|z| dt.with_timezone(&z).format("%Y-%m-%d %H:%M").to_string()),
            tz: parsed.map(|z| z.name().to_string()),
            unix,
        })
    }

    /// From an optional DB value (`NULL` / 0 → None).
    pub fn opt(unix: Option<i64>, tz: Option<&str>) -> Option<Stamp> {
        unix.filter(|u| *u > 0).and_then(|u| Stamp::new(u, tz))
    }

    /// Human form for tables: "Fri 23 Oct 11:00 +07".
    pub fn human(&self) -> String {
        let Some(dt) = Utc.timestamp_opt(self.unix, 0).single() else {
            return self.utc.clone();
        };
        match self.tz.as_deref().and_then(|t| t.parse::<Tz>().ok()) {
            Some(z) => dt
                .with_timezone(&z)
                .format("%a %d %b %Y %H:%M %:::z")
                .to_string(),
            None => dt.format("%a %d %b %Y %H:%M UTC").to_string(),
        }
    }
}

/// Local calendar date (YYYY-MM-DD) of a unix time in `tz` (UTC if unknown).
pub fn local_date(unix: i64, tz: Option<&str>) -> Option<String> {
    let dt = Utc.timestamp_opt(unix, 0).single()?;
    Some(match tz.and_then(|t| t.parse::<Tz>().ok()) {
        Some(z) => dt.with_timezone(&z).format("%Y-%m-%d").to_string(),
        None => dt.format("%Y-%m-%d").to_string(),
    })
}

/// "2h 10m"
pub fn duration(minutes: i64) -> String {
    let (h, m) = (minutes / 60, minutes % 60);
    match (h, m) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// "12 min late" / "3 min early" / "on time", from seconds of delay.
pub fn delay(seconds: i64) -> String {
    let m = seconds / 60;
    match m {
        0 => "on time".into(),
        m if m > 0 => format!("{m} min late"),
        m => format!("{} min early", -m),
    }
}

pub fn now_unix() -> i64 {
    Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_time_in_airport_zone() {
        // 2026-10-17 14:50 UTC = 16:50 Prague (CEST)
        let s = Stamp::new(1_792_248_600, Some("Europe/Prague")).unwrap();
        assert_eq!(s.utc, "2026-10-17T14:50:00Z");
        assert_eq!(s.local.as_deref(), Some("2026-10-17 16:50"));
        assert_eq!(s.tz.as_deref(), Some("Europe/Prague"));
    }

    #[test]
    fn humanises() {
        assert_eq!(duration(130), "2h 10m");
        assert_eq!(delay(720), "12 min late");
        assert_eq!(delay(-180), "3 min early");
    }
}
