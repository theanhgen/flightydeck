//! Connections (layovers) Flighty computed between the owner's flights.

use std::collections::HashMap;

use rusqlite::Connection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ctx::Ctx;
use crate::db;
use crate::error::Result;
use crate::render::{self, Render};
use crate::time::{self, Stamp};

/// Assumed minimum connection time when Flighty doesn't know one.
const DEFAULT_MCT_MINUTES: i64 = 45;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, clap::Args)]
pub struct ConnArgs {
    /// Only connections from now on.
    #[arg(long)]
    #[serde(default)]
    pub upcoming: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ConnectionInfo {
    pub airport: Option<String>,
    pub inbound: Option<String>,
    pub outbound: Option<String>,
    /// Inbound arrival (estimated, else scheduled), in the connecting airport's time zone.
    pub arrives: Option<Stamp>,
    /// Outbound departure (estimated, else scheduled), in the connecting airport's time zone.
    pub departs: Option<Stamp>,
    pub layover_minutes: Option<i64>,
    /// Minimum connection time if Flighty knows it.
    pub mct_minutes: Option<i64>,
    /// "ok" | "tight" | "missed" | "unknown"
    pub risk: String,
    pub steps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Connections {
    pub count: usize,
    pub connections: Vec<ConnectionInfo>,
}

impl Render for Connections {
    fn table(&self) -> String {
        let rows: Vec<Vec<String>> = self
            .connections
            .iter()
            .map(|c| {
                vec![
                    render::dash(c.airport.as_ref()),
                    render::dash(c.arrives.as_ref().map(Stamp::human)),
                    render::dash(c.inbound.as_ref()),
                    render::dash(c.outbound.as_ref()),
                    render::dash(c.layover_minutes.map(time::duration)),
                    render::dash(c.mct_minutes.map(time::duration)),
                    c.risk.clone(),
                ]
            })
            .collect();
        let mut out = render::table(
            &["Airport", "Arrives", "In", "Out", "Layover", "MCT", "Risk"],
            &rows,
        );
        for c in self.connections.iter().filter(|c| !c.steps.is_empty()) {
            out.push_str(&format!(
                "\n\n{} → {} at {}:",
                render::dash(c.inbound.as_ref()),
                render::dash(c.outbound.as_ref()),
                render::dash(c.airport.as_ref())
            ));
            for (i, s) in c.steps.iter().enumerate() {
                out.push_str(&format!("\n  {}. {s}", i + 1));
            }
        }
        out
    }
}

/// Risk of missing the outbound flight.
pub(crate) fn risk(layover: Option<i64>, mct: Option<i64>) -> &'static str {
    match layover {
        None => "unknown",
        Some(l) if l < 0 => "missed",
        Some(l) if l < mct.unwrap_or(DEFAULT_MCT_MINUTES) => "tight",
        Some(_) => "ok",
    }
}

// A flight id may point at Flight or ManualFlight; ManualFlight has no *Estimated columns.
const CONN_SQL: &str = r#"
SELECT c.id, c.mctMinutes,
       coalesce(w.iata, w.icao, w.name), w.timeZoneIdentifier,
       coalesce(ia.iata, ia.icao, mia.iata, mia.icao), coalesce(fi.number, mi.number),
       coalesce(oa.iata, oa.icao, moa.iata, moa.icao), coalesce(fo.number, mo.number),
       coalesce(nullif(fi.arrivalScheduleGateEstimated, 0), fi.arrivalScheduleGateOriginal,
                nullif(mi.arrivalScheduleGateActual, 0), mi.arrivalScheduleGateOriginal),
       coalesce(nullif(fo.departureScheduleGateEstimated, 0), fo.departureScheduleGateOriginal,
                nullif(mo.departureScheduleGateActual, 0), mo.departureScheduleGateOriginal)
FROM "Connection" c
LEFT JOIN Airport w ON w.id = c.waitingAirportId
LEFT JOIN Flight fi ON fi.id = c.arrivingFlightId
LEFT JOIN ManualFlight mi ON mi.id = c.arrivingFlightId
LEFT JOIN Airline ia ON ia.id = fi.airlineId
LEFT JOIN Airline mia ON mia.id = mi.airlineId
LEFT JOIN Flight fo ON fo.id = c.departingFlightId
LEFT JOIN ManualFlight mo ON mo.id = c.departingFlightId
LEFT JOIN Airline oa ON oa.id = fo.airlineId
LEFT JOIN Airline moa ON moa.id = mo.airlineId
WHERE c.userId = ?1 AND c.deleted IS NULL
"#;

fn code(prefix: Option<String>, number: Option<String>) -> Option<String> {
    number.map(|n| format!("{}{n}", prefix.unwrap_or_default()))
}

/// `ConnectionSteps.stepsData` blobs by connection id (newest row wins).
fn steps_by_connection(conn: &Connection, owner_id: &str) -> Result<HashMap<String, Vec<u8>>> {
    let mut stmt = conn.prepare(
        "SELECT connectionId, stepsData FROM ConnectionSteps
         WHERE userId = ?1 AND deleted IS NULL AND connectionId IS NOT NULL AND stepsData IS NOT NULL
         ORDER BY lastUpdated",
    )?;
    let rows = stmt.query_map([owner_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Connections for a known owner id (`list` resolves the owner first).
#[doc(hidden)]
pub fn list_for(conn: &Connection, owner_id: &str, a: &ConnArgs) -> Result<Connections> {
    let steps = steps_by_connection(conn, owner_id)?;
    let now = time::now_unix();
    let mut stmt = conn.prepare(CONN_SQL)?;
    let rows = stmt.query_map([owner_id], |r| {
        let id: String = r.get(0)?;
        let tz: Option<String> = r.get(3)?;
        let arr: Option<i64> = r.get::<_, Option<i64>>(8)?.filter(|t| *t > 0);
        let dep: Option<i64> = r.get::<_, Option<i64>>(9)?.filter(|t| *t > 0);
        let mct: Option<i64> = r.get(1)?;
        let layover = arr.zip(dep).map(|(a, d)| (d - a).div_euclid(60));
        Ok((
            dep,
            ConnectionInfo {
                airport: r.get(2)?,
                inbound: code(r.get(4)?, r.get(5)?),
                outbound: code(r.get(6)?, r.get(7)?),
                arrives: Stamp::opt(arr, tz.as_deref()),
                departs: Stamp::opt(dep, tz.as_deref()),
                layover_minutes: layover,
                mct_minutes: mct,
                risk: risk(layover, mct).to_string(),
                steps: steps.get(&id).map(|b| steps::decode(b)).unwrap_or_default(),
            },
        ))
    })?;
    let mut list: Vec<(Option<i64>, ConnectionInfo)> = rows
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|(dep, _)| !a.upcoming || dep.is_some_and(|d| d >= now))
        .collect();
    list.sort_by_key(|(dep, c)| {
        (
            c.arrives.as_ref().map(|s| s.unix).or(*dep),
            c.airport.clone(),
        )
    });
    let connections: Vec<ConnectionInfo> = list.into_iter().map(|(_, c)| c).collect();
    Ok(Connections {
        count: connections.len(),
        connections,
    })
}

pub fn list(ctx: &Ctx, a: &ConnArgs) -> Result<Connections> {
    let conn = db::open(ctx)?;
    let owner = crate::owner::resolve(ctx, &conn)?;
    list_for(&conn, &owner.user_id, a)
}

/// `stepsData` is a protobuf message: repeated field 8 = step, whose field 1 is the title and
/// field 2 the description. Decoding fails closed: anything unexpected yields no steps.
mod steps {
    fn varint(b: &[u8], i: &mut usize) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *b.get(*i)?;
            *i += 1;
            v |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }

    /// (field number, length-delimited payload or None for other wire types).
    type Field<'a> = (u64, Option<&'a [u8]>);

    fn fields(b: &[u8]) -> Option<Vec<Field<'_>>> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            let key = varint(b, &mut i)?;
            let (field, wire) = (key >> 3, key & 7);
            match wire {
                0 => {
                    varint(b, &mut i)?;
                    out.push((field, None));
                }
                1 => {
                    i = i.checked_add(8).filter(|e| *e <= b.len())?;
                    out.push((field, None));
                }
                2 => {
                    let len = usize::try_from(varint(b, &mut i)?).ok()?;
                    let end = i.checked_add(len).filter(|e| *e <= b.len())?;
                    out.push((field, Some(&b[i..end])));
                    i = end;
                }
                5 => {
                    i = i.checked_add(4).filter(|e| *e <= b.len())?;
                    out.push((field, None));
                }
                _ => return None,
            }
        }
        Some(out)
    }

    fn text(fields: &[Field<'_>], n: u64) -> Option<String> {
        fields
            .iter()
            .find(|(f, _)| *f == n)
            .and_then(|(_, p)| std::str::from_utf8((*p)?).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    pub(super) fn decode(blob: &[u8]) -> Vec<String> {
        let parse = || -> Option<Vec<String>> {
            let mut out = Vec::new();
            for (field, payload) in fields(blob)? {
                if field != 8 {
                    continue;
                }
                let step = fields(payload?)?;
                let title = text(&step, 1)?;
                out.push(match text(&step, 2) {
                    Some(d) => format!("{title}: {d}"),
                    None => title,
                });
            }
            Some(out)
        };
        parse().unwrap_or_default()
    }

    #[cfg(test)]
    mod tests {
        use super::decode;

        fn msg(field: u8, payload: &[u8]) -> Vec<u8> {
            let mut v = vec![(field << 3) | 2, payload.len() as u8];
            v.extend_from_slice(payload);
            v
        }

        #[test]
        fn decodes_steps() {
            let mut step1 = msg(1, b"Disembark");
            step1.extend(msg(2, b"Follow the transfer signs."));
            step1.extend([0x18, 10]); // varint field 3, ignored
            let step2 = msg(1, b"Security");
            let mut blob = msg(8, &step1);
            blob.extend(msg(8, &step2));
            assert_eq!(
                decode(&blob),
                vec!["Disembark: Follow the transfer signs.", "Security"]
            );
        }

        #[test]
        fn garbage_gives_no_steps() {
            assert!(decode(b"{\"not\": \"protobuf\"}").is_empty());
            assert!(decode(&[0x42, 0x7f, 0x01]).is_empty()); // length past the end
            assert!(decode(&[]).is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::risk;

    #[test]
    fn risk_levels() {
        assert_eq!(risk(None, Some(60)), "unknown");
        assert_eq!(risk(Some(-5), Some(60)), "missed");
        assert_eq!(risk(Some(50), Some(60)), "tight");
        assert_eq!(risk(Some(60), Some(60)), "ok");
        assert_eq!(risk(Some(40), None), "tight");
        assert_eq!(risk(Some(45), None), "ok");
    }
}
