//! Airport and airline lookup (accent-insensitive: "con dao" finds VCS).

use rusqlite::{Connection, ToSql};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;
use crate::db;
use crate::error::{Error, Result};
use crate::render::{self, Render};

const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 500;

#[derive(Debug, Clone, Deserialize, JsonSchema, clap::Args)]
pub struct LookupArgs {
    /// Name, city, IATA or ICAO code.
    pub query: String,
    /// Maximum results (default 20).
    #[arg(long)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Airport {
    pub iata: Option<String>,
    pub icao: Option<String>,
    pub name: Option<String>,
    pub city: Option<String>,
    pub country: Option<String>,
    pub tz: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Airline {
    /// Flighty airline UUID (what the search API wants).
    pub id: String,
    pub iata: Option<String>,
    pub icao: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Airports {
    pub count: usize,
    pub airports: Vec<Airport>,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Airlines {
    pub count: usize,
    pub airlines: Vec<Airline>,
}

impl Render for Airports {
    fn table(&self) -> String {
        let rows: Vec<Vec<String>> = self
            .airports
            .iter()
            .map(|a| {
                vec![
                    render::dash(a.iata.as_ref()),
                    render::dash(a.icao.as_ref()),
                    render::dash(a.name.as_ref()),
                    render::dash(a.city.as_ref()),
                    render::dash(a.country.as_ref()),
                    render::dash(a.tz.as_ref()),
                ]
            })
            .collect();
        render::table(
            &["IATA", "ICAO", "Name", "City", "Country", "Time zone"],
            &rows,
        )
    }
}
impl Render for Airlines {
    fn table(&self) -> String {
        let rows: Vec<Vec<String>> = self
            .airlines
            .iter()
            .map(|a| {
                vec![
                    render::dash(a.iata.as_ref()),
                    render::dash(a.icao.as_ref()),
                    render::dash(a.name.as_ref()),
                    a.id.clone(),
                ]
            })
            .collect();
        render::table(&["IATA", "ICAO", "Name", "Id"], &rows)
    }
}

/// Folded query split into words, plus the LIMIT to use.
fn prepare(a: &LookupArgs) -> Result<(String, Vec<String>, u32)> {
    let folded = db::fold(a.query.trim());
    let words: Vec<String> = folded.split_whitespace().map(str::to_string).collect();
    if words.is_empty() {
        return Err(Error::BadInput("search query is empty".into()));
    }
    let limit = a.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    Ok((words.join(" "), words, limit))
}

/// `%word%` with LIKE wildcards escaped (`\` is the escape character).
fn like_pattern(word: &str) -> String {
    let escaped = word
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

/// Shared search: an exact IATA/ICAO hit, or every query word found in `haystack`.
/// Exact code matches rank first, then Flighty's relevance.
fn search<T>(
    conn: &Connection,
    select: &str,
    table: &str,
    haystack: &str,
    a: &LookupArgs,
    map: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    let (code, words, limit) = prepare(a)?;
    let words_clause = (0..words.len())
        .map(|i| format!("fold({haystack}) LIKE ?{} ESCAPE '\\'", i + 3))
        .collect::<Vec<_>>()
        .join(" AND ");
    let sql = format!(
        "SELECT {select} FROM \"{table}\" t
         WHERE t.deleted IS NULL
           AND (fold(t.iata) = ?1 OR fold(t.icao) = ?1 OR ({words_clause}))
         ORDER BY (fold(t.iata) = ?1 OR fold(t.icao) = ?1) DESC, t.relevance DESC, t.name
         LIMIT ?2"
    );
    let patterns: Vec<String> = words.iter().map(|w| like_pattern(w)).collect();
    let mut params: Vec<&dyn ToSql> = vec![&code, &limit];
    params.extend(patterns.iter().map(|p| p as &dyn ToSql));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params.as_slice(), map)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub(crate) fn airports_in(conn: &Connection, a: &LookupArgs) -> Result<Airports> {
    let airports = search(
        conn,
        "t.iata, t.icao, t.name, t.city, t.country, t.timeZoneIdentifier",
        "Airport",
        "coalesce(t.name,'') || ' ' || coalesce(t.city,'') || ' ' || coalesce(t.iata,'') || ' ' || coalesce(t.icao,'')",
        a,
        |r| {
            Ok(Airport {
                iata: r.get(0)?,
                icao: r.get(1)?,
                name: r.get(2)?,
                city: r.get(3)?,
                country: r.get(4)?,
                tz: r.get(5)?,
            })
        },
    )?;
    Ok(Airports {
        count: airports.len(),
        airports,
    })
}

pub(crate) fn airlines_in(conn: &Connection, a: &LookupArgs) -> Result<Airlines> {
    let airlines = search(
        conn,
        "t.id, t.iata, t.icao, t.name",
        "Airline",
        "coalesce(t.name,'') || ' ' || coalesce(t.iata,'') || ' ' || coalesce(t.icao,'')",
        a,
        |r| {
            Ok(Airline {
                id: r.get(0)?,
                iata: r.get(1)?,
                icao: r.get(2)?,
                name: r.get(3)?,
            })
        },
    )?;
    Ok(Airlines {
        count: airlines.len(),
        airlines,
    })
}

pub fn airports(ctx: &Ctx, a: &LookupArgs) -> Result<Airports> {
    airports_in(&db::open(ctx)?, a)
}
pub fn airlines(ctx: &Ctx, a: &LookupArgs) -> Result<Airlines> {
    airlines_in(&db::open(ctx)?, a)
}

#[cfg(test)]
mod tests {
    use super::like_pattern;

    #[test]
    fn escapes_like_wildcards() {
        assert_eq!(like_pattern("a_b%c"), "%a\\_b\\%c%");
    }
}
