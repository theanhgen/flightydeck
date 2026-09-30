//! The only module that writes to stdout. Everything else logs to stderr.

use serde::Serialize;

/// Every op result implements this: JSON for machines (same shape as the MCP tool output),
/// a plain-text table for humans.
pub trait Render: Serialize {
    fn table(&self) -> String;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Table,
    Json,
}

pub fn to_json<T: Serialize + ?Sized>(v: &T) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|e| format!("{{\"error\":\"serialize: {e}\"}}"))
}

pub fn print<T: Render>(v: &T, fmt: Format) {
    match fmt {
        Format::Json => println!("{}", to_json(v)),
        Format::Table => println!("{}", v.table()),
    }
}

/// Simple left-aligned text table.
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let n = headers.len();
    let mut w: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate().take(n) {
            w[i] = w[i].max(c.chars().count());
        }
    }
    let line = |cells: Vec<&str>| -> String {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c:<width$}", width = w[i]))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    let mut out = vec![line(headers.to_vec())];
    out.push(
        w.iter()
            .map(|x| "-".repeat(*x))
            .collect::<Vec<_>>()
            .join("  "),
    );
    for r in rows {
        out.push(line(r.iter().map(String::as_str).collect()));
    }
    if rows.is_empty() {
        out.push("(none)".into());
    }
    out.join("\n")
}

/// "—" for missing values in tables.
pub fn dash(v: Option<impl ToString>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "—".into())
}
