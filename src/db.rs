//! Read-only access to Flighty's local SQLite DB.
//!
//! Rules: open per operation and drop right after (never hold a connection across calls:
//! the DB uses rollback-journal mode and a held SHARED lock blocks the app's writes);
//! read-only flags; 5 s busy timeout; never touch -journal/-wal files; never write.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, OpenFlags};
use unicode_normalization::UnicodeNormalization;

use crate::ctx::Ctx;
use crate::error::{Error, Result};

/// Open the main DB read-only with `fold()` registered.
pub fn open(ctx: &Ctx) -> Result<Connection> {
    check_readable(&ctx.db_path)?;
    let conn = Connection::open_with_flags(
        &ctx.db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA query_only = ON;")?;
    conn.create_scalar_function(
        "fold",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |c| {
            let s: Option<String> = c.get(0)?;
            Ok(s.map(|s| fold(&s)))
        },
    )?;
    Ok(conn)
}

/// Says why the database can't be read, before SQLite turns it into "unable to open
/// database file". macOS denies other apps' containers to a terminal that lacks the permission.
fn check_readable(path: &Path) -> Result<()> {
    let Err(e) = std::fs::File::open(path) else {
        return Ok(());
    };
    let at = path.display();
    Err(Error::NotReady(match e.kind() {
        ErrorKind::NotFound => format!(
            "Flighty database not found at {at}. Install the Flighty Mac app, open it and sign in once."
        ),
        ErrorKind::PermissionDenied => format!(
            "macOS is blocking this terminal from reading Flighty's data at {at}. \
             Open System Settings > Privacy & Security > Full Disk Access, turn it on for your \
             terminal app, then quit and reopen the terminal."
        ),
        _ => format!("can't read the Flighty database at {at}: {e}"),
    }))
}

/// Accent/case fold: NFD, drop combining marks, lower-case, map đ/Đ → d (NFD doesn't).
/// "Querétaro" → "queretaro", "Düsseldorf" → "dusseldorf".
pub fn fold(s: &str) -> String {
    s.nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .map(|c| match c {
            'đ' | 'Đ' => 'd',
            'ø' | 'Ø' => 'o',
            'ł' | 'Ł' => 'l',
            'ß' => 's',
            c => c,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

type ColCache = HashMap<(String, String), Vec<String>>;
static COLS: Mutex<Option<(Instant, ColCache)>> = Mutex::new(None);

/// Column names of `table` (cached 60 s so a long-lived MCP server notices app updates).
pub fn columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let key = (conn.path().unwrap_or("").to_string(), table.to_string());
    {
        let guard = COLS.lock().unwrap();
        if let Some((at, map)) = guard.as_ref()
            && at.elapsed() < Duration::from_secs(60)
            && let Some(v) = map.get(&key)
        {
            return Ok(v.clone());
        }
    }
    let mut stmt = conn.prepare("SELECT name FROM pragma_table_info(?1)")?;
    let cols: Vec<String> = stmt
        .query_map([table], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<_, _>>()?;
    let mut guard = COLS.lock().unwrap();
    let fresh = matches!(guard.as_ref(), Some((at, _)) if at.elapsed() < Duration::from_secs(60));
    if !fresh {
        *guard = Some((Instant::now(), HashMap::new()));
    }
    guard.as_mut().unwrap().1.insert(key, cols.clone());
    Ok(cols)
}

/// Whether `table.column` exists.
pub fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    Ok(columns(conn, table)?.iter().any(|c| c == column))
}

/// First existing column among `candidates` (newest name first), as a SQL expression
/// qualified with `alias`, or `NULL` if none exists. Lets a renamed optional column blank one
/// field instead of failing the query.
pub fn col_or_null(
    conn: &Connection,
    table: &str,
    alias: &str,
    candidates: &[&str],
) -> Result<String> {
    let cols = columns(conn, table)?;
    for c in candidates {
        if cols.iter().any(|x| x == c) {
            return Ok(format!("{alias}.\"{c}\""));
        }
    }
    Ok("NULL".into())
}

/// Drop the column cache (after a "no such column" error).
pub fn invalidate_cache() {
    *COLS.lock().unwrap() = None;
}

#[cfg(test)]
mod tests {
    use super::fold;

    #[test]
    fn folds_accents() {
        assert_eq!(fold("Querétaro"), "queretaro");
        assert_eq!(fold("Düsseldorf"), "dusseldorf");
        assert_eq!(fold("São Paulo"), "sao paulo");
        assert_eq!(fold("Đakovo"), "dakovo");
    }
}
