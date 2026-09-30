//! `about`: version, mode, DB/token presence, schema check. Never prints the token.

use rusqlite::Connection;
use serde::Serialize;

use crate::ctx::Ctx;
use crate::db;
use crate::error::Result;
use crate::render::{self, Render};

pub const DISCLAIMER: &str = "Unofficial tool. Not made by or affiliated with Flighty.";

/// Tables and columns the crate cannot work without. Optional columns (weather, belt, tail)
/// are looked up per query with `db::col_or_null` and are not listed here.
const REQUIRED: &[(&str, &[&str])] = &[
    (
        "Flight",
        &[
            "id",
            "number",
            "departureAirportId",
            "scheduledArrivalAirportId",
            "actualArrivalAirportId",
            "airlineId",
            "departureScheduleGateOriginal",
            "departureScheduleGateEstimated",
            "departureScheduleGateActual",
            "arrivalScheduleGateOriginal",
            "arrivalScheduleGateEstimated",
            "arrivalScheduleGateActual",
            "isCancelled",
            "distance",
            "deleted",
        ],
    ),
    (
        "ManualFlight",
        &[
            "id",
            "number",
            "departureAirportId",
            "scheduledArrivalAirportId",
            "actualArrivalAirportId",
            "airlineId",
            "departureScheduleGateOriginal",
            "departureScheduleGateActual",
            "arrivalScheduleGateOriginal",
            "arrivalScheduleGateActual",
            "isCancelled",
            "distance",
            "durationMinutes",
            "deleted",
        ],
    ),
    (
        "UserFlight",
        &["userId", "flightId", "isMyFlight", "isArchived", "deleted"],
    ),
    (
        "UserManualFlight",
        &["userId", "flightId", "isMyFlight", "isArchived", "deleted"],
    ),
    (
        "Airport",
        &[
            "id",
            "iata",
            "icao",
            "name",
            "city",
            "country",
            "countryCode",
            "timeZoneIdentifier",
            "relevance",
            "deleted",
        ],
    ),
    (
        "Airline",
        &["id", "iata", "icao", "name", "relevance", "deleted"],
    ),
    ("Account", &["id", "authToken", "rawKind"]),
    ("User", &["remoteId", "accountId"]),
    (
        "Connection",
        &[
            "id",
            "userId",
            "arrivingFlightId",
            "departingFlightId",
            "waitingAirportId",
            "mctMinutes",
            "deleted",
        ],
    ),
];

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct About {
    pub name: String,
    pub version: String,
    /// "read-write" | "read-only"
    pub mode: String,
    pub remove_enabled: bool,
    pub db_path: String,
    pub db_found: bool,
    pub token_found: bool,
    pub token_expired: Option<bool>,
    pub build_token_found: bool,
    pub app_version: Option<String>,
    pub owner_user_id: Option<String>,
    pub owner_source: Option<String>,
    /// Missing required columns, empty if the schema is usable.
    pub schema_missing: Vec<String>,
    pub schema_ok: bool,
    pub disclaimer: String,
}

impl Render for About {
    fn table(&self) -> String {
        let yes_no = |b: bool| if b { "yes" } else { "no" }.to_string();
        let token = match (self.token_found, self.token_expired) {
            (false, _) => "not found".to_string(),
            (true, Some(true)) => "found, expired (open Flighty to refresh it)".to_string(),
            (true, _) => "found".to_string(),
        };
        let owner = match (&self.owner_user_id, &self.owner_source) {
            (Some(id), Some(src)) => format!("{id} (from {src})"),
            (Some(id), None) => id.clone(),
            _ => "unresolved".into(),
        };
        let schema = if self.schema_ok {
            "ok".to_string()
        } else if !self.db_found {
            "not checked (no database)".to_string()
        } else {
            format!("missing: {}", self.schema_missing.join(", "))
        };
        let rows = vec![
            vec!["Version".into(), self.version.clone()],
            vec!["Mode".into(), self.mode.clone()],
            vec!["Remove enabled".into(), yes_no(self.remove_enabled)],
            vec![
                "Database".into(),
                format!(
                    "{} ({})",
                    self.db_path,
                    if self.db_found { "found" } else { "not found" }
                ),
            ],
            vec!["Token".into(), token],
            vec![
                "Build token".into(),
                if self.build_token_found {
                    "found"
                } else {
                    "not found"
                }
                .into(),
            ],
            vec![
                "Flighty app".into(),
                render::dash(self.app_version.as_ref()),
            ],
            vec!["Owner".into(), owner],
            vec!["Schema".into(), schema],
        ];
        format!(
            "{}\n\n{}",
            render::table(&[&self.name, ""], &rows),
            self.disclaimer
        )
    }
}

/// Required `Table.column`s absent from the DB (a missing table is reported as `Table`).
/// Reads `PRAGMA table_info` directly, bypassing the column cache, so it is always current.
pub fn schema_missing(conn: &Connection) -> Result<Vec<String>> {
    let mut missing = Vec::new();
    let mut stmt = conn.prepare("SELECT name FROM pragma_table_info(?1)")?;
    for (table, cols) in REQUIRED {
        let have: Vec<String> = stmt
            .query_map([table], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<_, _>>()?;
        if have.is_empty() {
            missing.push((*table).to_string());
            continue;
        }
        missing.extend(
            cols.iter()
                .filter(|c| !have.iter().any(|h| h == *c))
                .map(|c| format!("{table}.{c}")),
        );
    }
    Ok(missing)
}

/// Never fails: every problem is reported as a field.
pub fn about(ctx: &Ctx) -> Result<About> {
    let creds = crate::creds::status(ctx);
    let db_found = ctx.db_path.exists();
    let (mut owner, mut missing) = (None, Vec::new());
    if db_found {
        match db::open(ctx) {
            Ok(conn) => {
                missing = schema_missing(&conn)
                    .unwrap_or_else(|e| vec![format!("schema check failed: {e}")]);
                // An unresolvable owner is reported as None, not as an error.
                owner = crate::owner::resolve(ctx, &conn).ok();
            }
            Err(e) => missing.push(format!("database unreadable: {e}")),
        }
    }
    Ok(About {
        name: "flightydeck".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        mode: if ctx.read_only {
            "read-only"
        } else {
            "read-write"
        }
        .into(),
        remove_enabled: ctx.allows(crate::ctx::Class::Destructive),
        db_path: ctx.db_path.display().to_string(),
        db_found,
        token_found: creds.token_found,
        token_expired: creds.token_expired,
        build_token_found: creds.build_token_found,
        app_version: creds.app_version,
        owner_user_id: owner.as_ref().map(|o| o.user_id.clone()),
        owner_source: owner.map(|o| o.source.to_string()),
        schema_ok: db_found && missing.is_empty(),
        schema_missing: missing,
        disclaimer: DISCLAIMER.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_schema() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../tests/fixtures/schema.sql"))
            .unwrap();
        conn
    }

    #[test]
    fn fixture_schema_is_complete() {
        assert_eq!(
            schema_missing(&fixture_schema()).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn reports_missing_column_and_table() {
        let conn = fixture_schema();
        conn.execute_batch("ALTER TABLE Flight DROP COLUMN distance; DROP TABLE \"Connection\";")
            .unwrap();
        let missing = schema_missing(&conn).unwrap();
        assert_eq!(
            missing,
            vec!["Flight.distance".to_string(), "Connection".to_string()]
        );
    }
}
