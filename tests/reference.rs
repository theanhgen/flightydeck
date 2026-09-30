mod common;

use flightydeck::ops::reference::{self, LookupArgs};
use flightydeck::render::Render;

fn q(query: &str) -> LookupArgs {
    LookupArgs {
        query: query.into(),
        limit: None,
    }
}

fn airport_codes(query: &str) -> Vec<String> {
    let fx = common::fixture();
    reference::airports(&fx.ctx(), &q(query))
        .unwrap()
        .airports
        .into_iter()
        .filter_map(|a| a.iata)
        .collect()
}

#[test]
fn accent_insensitive_airports() {
    assert_eq!(airport_codes("con dao"), vec!["VCS"]);
    assert_eq!(airport_codes("zurich"), vec!["ZRH"]);
    assert_eq!(airport_codes("Côn Đảo"), vec!["VCS"]);
    assert_eq!(airport_codes("ho chi minh"), vec!["SGN"]);
}

#[test]
fn exact_code_ranks_first() {
    // "han" is HAN's code; no other fixture airport contains it, but the code hit must lead.
    assert_eq!(
        airport_codes("han").first().map(String::as_str),
        Some("HAN")
    );
    assert_eq!(airport_codes("lkpr"), vec!["PRG"]);
}

#[test]
fn skips_deleted() {
    assert!(airport_codes("closed test").is_empty());
    assert!(airport_codes("ZZD").is_empty());
    let fx = common::fixture();
    assert_eq!(reference::airlines(&fx.ctx(), &q("ZZ")).unwrap().count, 0);
}

#[test]
fn airlines_by_code_returns_every_match() {
    let fx = common::fixture();
    let res = reference::airlines(&fx.ctx(), &q("QR")).unwrap();
    let names: Vec<_> = res.airlines.iter().filter_map(|a| a.name.clone()).collect();
    assert_eq!(names, vec!["Qatar Airways", "Qatar Executive"]);
    assert!(res.airlines.iter().all(|a| !a.id.is_empty()));
}

#[test]
fn limit_and_empty_query() {
    let fx = common::fixture();
    let res = reference::airports(
        &fx.ctx(),
        &LookupArgs {
            query: "international".into(),
            limit: Some(2),
        },
    )
    .unwrap();
    assert_eq!(res.count, 2);
    let err = reference::airports(&fx.ctx(), &q("   ")).unwrap_err();
    assert_eq!(err.exit_code(), 2);
}

#[test]
fn renders_tables() {
    let fx = common::fixture();
    let t = reference::airports(&fx.ctx(), &q("zurich"))
        .unwrap()
        .table();
    assert!(t.contains("ZRH") && t.contains("Europe/Zurich"));
    let t = reference::airlines(&fx.ctx(), &q("swiss")).unwrap().table();
    assert!(t.contains("LX") && t.contains("SWR"));
}
