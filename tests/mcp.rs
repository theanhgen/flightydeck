//! `flightydeck mcp` over stdio JSON-RPC: tool registration per policy, tool call output.

mod common;

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

const READ_TOOLS: &[&str] = &[
    "flightydeck_list_flights",
    "flightydeck_get_flight",
    "flightydeck_search_flights",
    "flightydeck_current_flights",
    "flightydeck_get_flight_status",
    "flightydeck_get_delay_forecast",
    "flightydeck_list_friend_flights",
    "flightydeck_get_flight_stats",
    "flightydeck_get_connections",
    "flightydeck_search_airports",
    "flightydeck_search_airlines",
    "flightydeck_about",
    "flightydeck_export_ics",
];
const WRITE_TOOLS: &[&str] = &["flightydeck_add_flight", "flightydeck_follow_flight"];
const REMOVE_TOOL: &str = "flightydeck_remove_flight";

/// A running `flightydeck mcp` with an initialized session.
struct Server {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
}

impl Server {
    fn start(db: &Path, env: &[(&str, &str)]) -> Server {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_flightydeck"));
        cmd.arg("mcp")
            .env("FLIGHTY_DB", db)
            .env("FLIGHTY_APP_PLIST", "/nonexistent/Info.plist")
            .env_remove("FLIGHTY_READ_ONLY")
            .env_remove("FLIGHTY_ALLOW_REMOVE")
            .env_remove("FLIGHTY_OWNER_USER_ID")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn flightydeck mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut s = Server {
            child,
            stdin,
            lines,
            next_id: 1,
        };
        let init = s.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "flightydeck-test", "version": "0" }
            }),
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "flightydeck");
        assert!(
            init["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("nofficial")
        );
        s.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        s
    }

    fn send(&mut self, msg: &Value) {
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// Sends a request and returns the response with the same id. Every stdout line must be
    /// JSON-RPC: anything else means something other than the transport wrote to stdout.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(20))
                .unwrap_or_else(|_| panic!("no response to {method}"));
            let msg: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("non-JSON on stdout ({e}): {line}"));
            assert_eq!(msg["jsonrpc"], "2.0", "{line}");
            if msg["id"] == json!(id) {
                return msg;
            }
        }
    }

    fn tool_names(&mut self) -> BTreeSet<String> {
        let res = self.request("tools/list", json!({}));
        res["result"]["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }

    /// The tool result; its text content parsed as JSON.
    fn call(&mut self, name: &str, arguments: Value) -> (bool, Value) {
        let res = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        let result = &res["result"];
        let text = result["content"][0]["text"].as_str().expect("text content");
        let is_error = result["isError"].as_bool().unwrap_or(false);
        (
            is_error,
            serde_json::from_str(text).expect("tool output is JSON"),
        )
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn expected(extra: &[&str]) -> BTreeSet<String> {
    READ_TOOLS
        .iter()
        .chain(extra)
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn default_mode_has_reads_and_writes_but_no_remove() {
    let fx = common::fixture();
    let mut s = Server::start(&fx.db, &[]);
    assert_eq!(s.tool_names(), expected(WRITE_TOOLS));
}

#[test]
fn read_only_mode_has_only_reads() {
    let fx = common::fixture();
    let mut s = Server::start(&fx.db, &[("FLIGHTY_READ_ONLY", "1")]);
    assert_eq!(s.tool_names(), expected(&[]));
}

#[test]
fn allow_remove_registers_remove() {
    let fx = common::fixture();
    let mut s = Server::start(&fx.db, &[("FLIGHTY_ALLOW_REMOVE", "1")]);
    let mut all = WRITE_TOOLS.to_vec();
    all.push(REMOVE_TOOL);
    assert_eq!(s.tool_names(), expected(&all));
}

#[test]
fn read_only_overrides_allow_remove() {
    let fx = common::fixture();
    let mut s = Server::start(
        &fx.db,
        &[("FLIGHTY_READ_ONLY", "1"), ("FLIGHTY_ALLOW_REMOVE", "1")],
    );
    assert_eq!(s.tool_names(), expected(&[]));
}

#[test]
fn remove_without_yes_is_a_tool_error() {
    let fx = common::fixture();
    let mut s = Server::start(&fx.db, &[("FLIGHTY_ALLOW_REMOVE", "1")]);
    let (is_error, out) = s.call(
        REMOVE_TOOL,
        json!({ "id": "e0000000-0000-4000-8000-000000000001" }),
    );
    assert!(is_error);
    assert_eq!(out["error"]["kind"], "refused");
}

#[test]
fn unregistered_tool_is_not_callable() {
    let fx = common::fixture();
    let mut s = Server::start(&fx.db, &[("FLIGHTY_READ_ONLY", "1")]);
    let res = s.request(
        "tools/call",
        json!({ "name": "flightydeck_add_flight", "arguments": { "flight": "AM333", "date": "2031-03-14" } }),
    );
    let refused = res.get("error").is_some() || res["result"]["isError"] == json!(true);
    assert!(refused, "{res}");
}

#[test]
fn about_tool_returns_the_about_json() {
    let fx = common::fixture();
    let mut s = Server::start(&fx.db, &[("FLIGHTY_READ_ONLY", "1")]);
    let (is_error, out) = s.call("flightydeck_about", json!({}));
    assert!(!is_error, "{out}");
    assert_eq!(out["mode"], "read-only");
    assert_eq!(out["db_found"], true);
}

#[test]
fn list_tool_matches_cli_json_shape() {
    let fx = common::fixture();
    let mut s = Server::start(&fx.db, &[]);
    let (is_error, out) = s.call("flightydeck_list_flights", json!({}));
    assert!(!is_error, "{out}");
    assert_eq!(out["owner_user_id"], common::OWNER);
    assert!(!out["flights"].as_array().unwrap().is_empty());
}

#[test]
fn op_errors_are_tool_errors_with_kind() {
    let mut s = Server::start(
        Path::new("/nonexistent/flightydeck-test/MainFlightyDatabase.db"),
        &[],
    );
    let (is_error, out) = s.call("flightydeck_list_flights", json!({}));
    assert!(is_error);
    assert_eq!(out["error"]["kind"], "not_ready");
}
