//! End-to-end tests of the `flightydeck` binary: help, exit codes, JSON errors.

mod common;

use std::path::Path;
use std::process::{Command, Output, Stdio};

const SUBCOMMANDS: &[&str] = &[
    "list",
    "get",
    "search",
    "current",
    "status",
    "delay",
    "ics",
    "friends",
    "stats",
    "connections",
    "airports",
    "airlines",
    "add",
    "follow",
    "remove",
    "about",
    "mcp",
];

/// The binary with a clean policy environment and the DB pointed at `db`. stdin is never a TTY.
fn flightydeck(db: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_flightydeck"));
    c.env("FLIGHTY_DB", db)
        .env("FLIGHTY_APP_PLIST", "/nonexistent/Info.plist")
        .env_remove("FLIGHTY_READ_ONLY")
        .env_remove("FLIGHTY_ALLOW_REMOVE")
        .env_remove("FLIGHTY_OWNER_USER_ID")
        .stdin(Stdio::null());
    c
}

fn run(cmd: &mut Command) -> Output {
    cmd.output().expect("run flightydeck")
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn missing_db() -> std::path::PathBuf {
    Path::new("/nonexistent/flightydeck-test/MainFlightyDatabase.db").to_path_buf()
}

#[test]
fn help_works_for_every_subcommand() {
    let out = run(flightydeck(&missing_db()).arg("--help"));
    assert!(out.status.success());
    let top = String::from_utf8_lossy(&out.stdout);
    for sub in SUBCOMMANDS {
        assert!(top.contains(sub), "top-level help lists {sub}");
        let out = run(flightydeck(&missing_db()).args([sub, "--help"]));
        assert!(
            out.status.success(),
            "{sub} --help: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!out.stdout.is_empty(), "{sub} --help prints help");
    }
}

#[test]
fn long_help_has_examples() {
    for sub in ["add", "get", "ics", "remove"] {
        let out = run(flightydeck(&missing_db()).args([sub, "--help"]));
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("Examples:"),
            "{sub}"
        );
    }
}

#[test]
fn version_flag() {
    let out = run(flightydeck(&missing_db()).arg("--version"));
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn unknown_subcommand_is_usage_error() {
    let out = run(flightydeck(&missing_db()).arg("nope"));
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn remove_is_refused_without_opt_in() {
    let out = run(flightydeck(&missing_db()).args([
        "remove",
        "e0000000-0000-4000-8000-000000000001",
        "--yes",
    ]));
    assert_eq!(out.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&out.stderr).contains("FLIGHTY_ALLOW_REMOVE"));
}

#[test]
fn read_only_refuses_remove_even_when_allowed() {
    let out = run(flightydeck(&missing_db())
        .env("FLIGHTY_READ_ONLY", "1")
        .env("FLIGHTY_ALLOW_REMOVE", "1")
        .args(["remove", "e0000000-0000-4000-8000-000000000001", "--yes"]));
    assert_eq!(out.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&out.stderr).contains("read-only"));
}

#[test]
fn remove_without_yes_off_tty_is_refused_with_json_error() {
    let out = run(flightydeck(&missing_db())
        .env("FLIGHTY_ALLOW_REMOVE", "1")
        .args([
            "-o",
            "json",
            "remove",
            "e0000000-0000-4000-8000-000000000001",
        ]));
    assert_eq!(out.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("error: "));
    let v = json(&out);
    assert_eq!(v["error"]["kind"], "refused");
    assert!(v["error"]["message"].as_str().unwrap().contains("--yes"));
}

#[test]
fn list_without_db_is_not_ready() {
    let out = run(flightydeck(&missing_db()).args(["-o", "json", "list"]));
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(json(&out)["error"]["kind"], "not_ready");
}

#[test]
fn about_without_db_reports_it() {
    let out = run(flightydeck(&missing_db()).args(["-o", "json", "about"]));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    assert_eq!(v["db_found"], false);
    assert_eq!(v["mode"], "read-write");
}

#[test]
fn about_reports_read_only_mode() {
    let fx = common::fixture();
    let out = run(flightydeck(&fx.db)
        .env("FLIGHTY_READ_ONLY", "1")
        .args(["-o", "json", "about"]));
    assert!(out.status.success());
    let v = json(&out);
    assert_eq!(v["mode"], "read-only");
    assert_eq!(v["db_found"], true);
}

#[test]
fn list_on_fixture_returns_flights_json() {
    let fx = common::fixture();
    let out = run(flightydeck(&fx.db).args(["-o", "json", "list"]));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    assert_eq!(v["owner_user_id"], common::OWNER);
    let flights = v["flights"].as_array().unwrap();
    assert!(!flights.is_empty());
    assert_eq!(v["count"].as_u64().unwrap() as usize, flights.len());
}

#[test]
fn list_on_fixture_prints_table() {
    let fx = common::fixture();
    let out = run(flightydeck(&fx.db).arg("list"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.stdout.is_empty());
    assert!(
        serde_json::from_slice::<serde_json::Value>(&out.stdout).is_err(),
        "table, not JSON"
    );
}

#[test]
fn ics_prints_a_calendar_file() {
    let fx = common::fixture();
    let out = run(flightydeck(&fx.db).args(["ics", "--upcoming"]));
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("BEGIN:VCALENDAR\r\n"));
    assert!(text.ends_with("END:VCALENDAR\r\n"), "no extra newline");
    assert_eq!(text.matches("BEGIN:VEVENT").count(), 5);

    let out = run(flightydeck(&fx.db).args(["-o", "json", "ics", "--upcoming"]));
    let v = json(&out);
    assert_eq!(v["count"], 5);
    assert_eq!(v["ics"], text);
}

#[test]
fn ics_date_needs_a_flight() {
    let out = run(flightydeck(&missing_db()).args(["ics", "--date", "2031-03-17"]));
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn add_with_bad_date_is_bad_input() {
    let fx = common::fixture();
    let out = run(flightydeck(&fx.db).args(["-o", "json", "add", "VN333", "2026-13-45"]));
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json(&out)["error"]["kind"], "bad_input");
}

#[test]
fn add_in_read_only_mode_is_refused() {
    let fx = common::fixture();
    let out =
        run(flightydeck(&fx.db)
            .env("FLIGHTY_READ_ONLY", "1")
            .args(["add", "VN333", "2026-10-14"]));
    assert_eq!(out.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&out.stderr).contains("read-only"));
}

#[test]
fn follow_in_read_only_mode_is_refused() {
    let fx = common::fixture();
    let out = run(flightydeck(&fx.db).env("FLIGHTY_READ_ONLY", "1").args([
        "follow",
        "VN333",
        "2026-10-14",
    ]));
    assert_eq!(out.status.code(), Some(5));
}
