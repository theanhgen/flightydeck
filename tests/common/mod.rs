//! Synthetic fixture DB: `tests/fixtures/schema.sql` (Flighty's DDL, no data) plus every
//! `tests/fixtures/seed_*.sql` in name order. All rows are invented. Never derive fixtures
//! from a real Flighty database.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use flightydeck::Ctx;

pub const OWNER: &str = "00000000-0000-4000-8000-000000000001";
pub const FRIEND: &str = "00000000-0000-4000-8000-000000000002";

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub db: PathBuf,
}

impl Fixture {
    pub fn ctx(&self) -> Ctx {
        Ctx::with_db(&self.db)
    }
}

pub fn fixture() -> Fixture {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("MainFlightyDatabase.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(&std::fs::read_to_string(root.join("schema.sql")).unwrap())
        .unwrap();
    let mut seeds: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("seed_") && n.ends_with(".sql"))
        })
        .collect();
    seeds.sort();
    for s in seeds {
        conn.execute_batch(&std::fs::read_to_string(&s).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", s.display()));
    }
    drop(conn);
    Fixture { dir, db }
}
