mod common;

use flightydeck::ops::connections::{self, ConnArgs};
use flightydeck::render::Render;

use common::OWNER;

fn run(upcoming: bool) -> connections::Connections {
    let fx = common::fixture();
    let conn = flightydeck::db::open(&fx.ctx()).unwrap();
    connections::list_for(&conn, OWNER, &ConnArgs { upcoming }).unwrap()
}

#[test]
fn lists_owner_connections_skipping_deleted() {
    let c = run(false);
    assert_eq!(c.count, 3);
    let airports: Vec<_> = c
        .connections
        .iter()
        .filter_map(|c| c.airport.clone())
        .collect();
    // Ordered by inbound arrival: the 2025 one first.
    assert_eq!(airports, vec!["LHR", "MAD", "GDL"]);
}

#[test]
fn layover_mct_and_risk() {
    let c = run(false);
    let mad = &c.connections[1];
    assert_eq!(mad.inbound.as_deref(), Some("IB111"));
    assert_eq!(mad.outbound.as_deref(), Some("IB222"));
    assert_eq!(mad.layover_minutes, Some(90));
    assert_eq!(mad.mct_minutes, Some(60));
    assert_eq!(mad.risk, "ok");
    assert_eq!(
        mad.arrives.as_ref().and_then(|s| s.tz.as_deref()),
        Some("Europe/Madrid")
    );

    let gdl = &c.connections[2];
    assert_eq!((gdl.layover_minutes, gdl.risk.as_str()), (Some(125), "ok"));

    // Into a manual flight that left before the inbound landed.
    let lhr = &c.connections[0];
    assert_eq!(lhr.outbound.as_deref(), Some("LH1480"));
    assert_eq!(lhr.mct_minutes, None);
    assert_eq!(lhr.risk, "missed");
}

#[test]
fn decodes_steps() {
    let c = run(false);
    assert_eq!(
        c.connections[1].steps,
        vec!["Disembark: Follow signs.", "Security"]
    );
    assert!(c.connections[2].steps.is_empty());
}

#[test]
fn upcoming_drops_past() {
    let c = run(true);
    assert_eq!(c.count, 2);
    assert!(
        c.connections
            .iter()
            .all(|c| c.airport.as_deref() != Some("LHR"))
    );
}

#[test]
fn other_user_sees_none() {
    let fx = common::fixture();
    let conn = flightydeck::db::open(&fx.ctx()).unwrap();
    let c = connections::list_for(&conn, common::FRIEND, &ConnArgs::default()).unwrap();
    assert_eq!(c.count, 0);
}

#[test]
fn renders_table() {
    let t = run(false).table();
    assert!(t.contains("IB111") && t.contains("1h 30m") && t.contains("missed"));
    assert!(t.contains("1. Disembark"));
}

#[test]
fn public_entry_point_resolves_owner() {
    let fx = common::fixture();
    assert_eq!(
        connections::list(&fx.ctx(), &ConnArgs::default())
            .unwrap()
            .count,
        3
    );
}
