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
    assert_eq!(airport_codes("queretaro"), vec!["QRO"]);
    assert_eq!(airport_codes("dusseldorf"), vec!["DUS"]);
    assert_eq!(airport_codes("Querétaro"), vec!["QRO"]);
    assert_eq!(airport_codes("guadalajara"), vec!["GDL"]);
}

#[test]
fn exact_code_ranks_first() {
    // "mex" is MEX's code and also the start of its city name; the code hit must lead.
    assert_eq!(
        airport_codes("mex").first().map(String::as_str),
        Some("MEX")
    );
    assert_eq!(airport_codes("egll"), vec!["LHR"]);
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
    let res = reference::airlines(&fx.ctx(), &q("IB")).unwrap();
    let names: Vec<_> = res.airlines.iter().filter_map(|a| a.name.clone()).collect();
    assert_eq!(names, vec!["Iberia", "Iberia Charter"]);
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
    let t = reference::airports(&fx.ctx(), &q("dusseldorf"))
        .unwrap()
        .table();
    assert!(t.contains("DUS") && t.contains("Europe/Berlin"));
    let t = reference::airlines(&fx.ctx(), &q("lufthansa"))
        .unwrap()
        .table();
    assert!(t.contains("LH") && t.contains("DLH"));
}
