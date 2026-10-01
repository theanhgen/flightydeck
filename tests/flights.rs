mod common;

use common::{OWNER, fixture};
use flightydeck::Error;
use flightydeck::model::Relation;
use flightydeck::ops::flights::{self, CurrentArgs, GetArgs, ListArgs, SearchArgs};
use flightydeck::render::Render;

const F: &str = "f0000000-0000-4000-8000-0000000000";
fn fid(n: u32) -> String {
    format!("{F}{n:02}")
}
const MANUAL: &str = "e0000000-0000-4000-8000-000000000001";

fn ids(list: &flights::FlightList) -> Vec<String> {
    list.flights.iter().map(|f| f.id.clone()).collect()
}

#[test]
fn list_defaults_to_own_unarchived_sorted() {
    let fx = fixture();
    let list = flights::list(&fx.ctx(), &ListArgs::default()).unwrap();
    assert_eq!(list.owner_user_id, OWNER);
    let got = ids(&list);
    // f06 archived, f07 followed, f08 friend's.
    let want = vec![fid(5), fid(10), fid(1), fid(2), fid(3), fid(4), fid(9)];
    let mut want_all = vec![MANUAL.to_string()];
    want_all.extend(want);
    assert_eq!(got, want_all);
    assert_eq!(list.count, 8);
    assert!(list.flights.iter().all(|f| f.relation == Relation::Mine));
}

#[test]
fn include_archived_and_following_but_never_friends() {
    let fx = fixture();
    let a = ListArgs {
        include_archived: true,
        include_following: true,
        ..Default::default()
    };
    let got = ids(&flights::list(&fx.ctx(), &a).unwrap());
    assert!(got.contains(&fid(6)));
    assert!(got.contains(&fid(7)));
    assert!(!got.contains(&fid(8)));
}

#[test]
fn archive_flag_ignored_when_old_app_archived_everything() {
    let fx = fixture();
    rusqlite::Connection::open(&fx.db)
        .unwrap()
        .execute_batch(&format!(
            "UPDATE UserFlight SET isArchived = 1 WHERE flightId IN ('{}','{}');
             UPDATE UserManualFlight SET isArchived = 1;",
            fid(5),
            fid(10)
        ))
        .unwrap();
    let got = ids(&flights::list(&fx.ctx(), &ListArgs::default()).unwrap());
    assert!(
        got.contains(&fid(6)),
        "all past flights archived → flag ignored"
    );
    assert!(got.contains(&fid(5)));
}

#[test]
fn upcoming_past_and_limit() {
    let fx = fixture();
    let ctx = fx.ctx();
    let up = flights::list(
        &ctx,
        &ListArgs {
            upcoming: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(ids(&up), vec![fid(1), fid(2), fid(3), fid(4), fid(9)]);
    let past = flights::list(
        &ctx,
        &ListArgs {
            past: true,
            limit: Some(2),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        ids(&past),
        vec![fid(5), fid(10)],
        "past keeps the most recent"
    );
}

#[test]
fn year_filter() {
    let fx = fixture();
    let year = |y| {
        ids(&flights::list(
            &fx.ctx(),
            &ListArgs {
                year: Some(y),
                ..Default::default()
            },
        )
        .unwrap())
    };
    // 2025: the manual flight and f05 (f06 is archived).
    assert_eq!(year(2025), vec![MANUAL.to_string(), fid(5)]);
    assert_eq!(year(2031), vec![fid(1), fid(2), fid(3), fid(4), fid(9)]);
    assert!(year(1999).is_empty());
}

#[test]
fn offset_pages_without_gaps_or_repeats() {
    let fx = fixture();
    let ctx = fx.ctx();
    let page = |upcoming, past, offset| {
        ids(&flights::list(
            &ctx,
            &ListArgs {
                upcoming,
                past,
                limit: Some(2),
                offset: Some(offset),
                ..Default::default()
            },
        )
        .unwrap())
    };
    // Upcoming pages run forward in time.
    assert_eq!(page(true, false, 0), vec![fid(1), fid(2)]);
    assert_eq!(page(true, false, 2), vec![fid(3), fid(4)]);
    assert_eq!(page(true, false, 4), vec![fid(9)]);
    assert!(page(true, false, 5).is_empty());
    assert!(page(true, false, 99).is_empty());
    // Past pages run backward from the most recent.
    assert_eq!(page(false, true, 0), vec![fid(5), fid(10)]);
    assert_eq!(page(false, true, 2), vec![MANUAL.to_string()]);
}

#[test]
fn manual_flight_included_with_duration() {
    let fx = fixture();
    let list = flights::list(&fx.ctx(), &ListArgs::default()).unwrap();
    let m = list.flights.iter().find(|f| f.id == MANUAL).unwrap();
    assert!(m.manual);
    assert_eq!(m.flight_code, "LX1480");
    assert_eq!(m.duration_minutes, Some(75));
    assert_eq!(m.from.iata.as_deref(), Some("PRG"));
}

#[test]
fn flight_fields_and_ticket() {
    let fx = fixture();
    let f = flights::get(
        &fx.ctx(),
        &GetArgs {
            flight: "qr 111".into(),
            date: None,
        },
    )
    .unwrap();
    assert_eq!(f.id, fid(1));
    assert_eq!(f.flight_code, "QR111");
    assert_eq!(f.airline_name.as_deref(), Some("Qatar Airways"));
    assert_eq!(f.duration_minutes, Some(350));
    let dep = f.departure_scheduled.as_ref().unwrap();
    assert_eq!(dep.utc, "2031-03-17T14:50:00Z");
    assert_eq!(dep.local.as_deref(), Some("2031-03-17 15:50"));
    assert_eq!(f.to.tz.as_deref(), Some("Asia/Qatar"));
    let t = f.ticket.as_ref().unwrap();
    assert_eq!(
        (t.booking_reference.as_deref(), t.seat.as_deref()),
        (Some("TEST01"), Some("32A"))
    );
    let table = f.table();
    assert!(table.contains("17 Mar 2031 15:50 +01"), "{table}");
    assert!(table.contains("5h 50m"), "{table}");
}

#[test]
fn get_nearest_and_by_date() {
    let fx = fixture();
    let ctx = fx.ctx();
    let get = |flight: &str, date: Option<&str>| {
        flights::get(
            &ctx,
            &GetArgs {
                flight: flight.into(),
                date: date.map(Into::into),
            },
        )
    };
    // Both LX1486 flights are past: default is the most recent (archived ones still count).
    assert_eq!(get("LX1486", None).unwrap().id, fid(6));
    assert_eq!(get("lx-1486", Some("2025-05-01")).unwrap().id, fid(5));
    assert_eq!(
        get("1486", None).unwrap().id,
        fid(6),
        "unambiguous bare number"
    );
    assert_eq!(get(&fid(3), None).unwrap().flight_code, "VN333");
    assert!(
        matches!(get("LX1486", Some("2025-05-02")), Err(Error::NotFound(m)) if m.contains("2025-05-01"))
    );
    assert!(matches!(get("QR999", None), Err(Error::NotFound(_))));
    assert!(matches!(
        get("QR111", Some("17/03/2031")),
        Err(Error::BadInput(_))
    ));
    assert!(matches!(get("hello", None), Err(Error::BadInput(_))));
    // Followed flights are searchable; friends' are not.
    assert_eq!(get("QR555", None).unwrap().relation, Relation::Following);
    assert!(matches!(get("VN666", None), Err(Error::NotFound(_))));
}

#[test]
fn bare_number_ambiguity_lists_options() {
    let fx = fixture();
    rusqlite::Connection::open(&fx.db)
        .unwrap()
        .execute_batch(&format!(
            "UPDATE Flight SET number = '1486' WHERE id = '{}';",
            fid(9)
        ))
        .unwrap();
    let err = flights::get(
        &fx.ctx(),
        &GetArgs {
            flight: "1486".into(),
            date: None,
        },
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(matches!(err, Error::BadInput(_)));
    assert!(msg.contains("LX1486") && msg.contains("VN1486"), "{msg}");
}

#[test]
fn accent_insensitive_search() {
    let fx = fixture();
    let ctx = fx.ctx();
    let search = |a: SearchArgs| ids(&flights::search(&ctx, &a).unwrap());
    let q = |s: &str| SearchArgs {
        query: Some(s.into()),
        ..Default::default()
    };

    assert_eq!(search(q("con dao")), vec![fid(4), fid(9)]);
    assert_eq!(
        search(q("zurich")),
        vec![MANUAL.to_string(), fid(5), fid(6)]
    );
    assert_eq!(search(q("ZÜRICH")), search(q("zurich")));
    assert_eq!(search(q("qr 111")), vec![fid(1)]);
    assert_eq!(search(q("Ha Noi")), vec![fid(2), fid(3)]);

    let from_to = SearchArgs {
        from: Some("sgn".into()),
        to: Some("côn đảo".into()),
        ..Default::default()
    };
    assert_eq!(search(from_to), vec![fid(4)]);
    let airline = SearchArgs {
        airline: Some("swiss".into()),
        after: Some("2025-04-01".into()),
        ..Default::default()
    };
    assert_eq!(search(airline), vec![fid(5), fid(6)]);
    let before = SearchArgs {
        airline: Some("LX".into()),
        before: Some("2025-03-01".into()),
        ..Default::default()
    };
    assert_eq!(search(before), vec![MANUAL.to_string()]);
    assert_eq!(search(q("100%")), Vec::<String>::new());
}

#[test]
fn current_shows_flight_in_the_air() {
    let fx = fixture();
    let got = ids(&flights::current(&fx.ctx(), &CurrentArgs::default()).unwrap());
    assert_eq!(got, vec![fid(10)]);
}

#[test]
fn cancelled_flagged_in_table() {
    let fx = fixture();
    let list = flights::list(&fx.ctx(), &ListArgs::default()).unwrap();
    let f = list.flights.iter().find(|f| f.id == fid(9)).unwrap();
    assert!(f.cancelled);
    let table = list.table();
    let line = table.lines().find(|l| l.contains("VN7777")).unwrap();
    assert!(line.contains("[CANCELLED]"), "{line}");
    assert!(table.contains("VCS → SGN"));
}

#[test]
fn delays_rendered() {
    let fx = fixture();
    let a = ListArgs {
        include_archived: true,
        past: true,
        ..Default::default()
    };
    let table = flights::list(&fx.ctx(), &a).unwrap().table();
    let line = table
        .lines()
        .find(|l| l.contains("2025-05") || l.contains("01 May 2025"))
        .unwrap();
    assert!(
        line.contains("dep 20 min late") && line.contains("arr 22 min late"),
        "{line}"
    );
}

#[test]
fn json_has_no_token() {
    let fx = fixture();
    let a = ListArgs {
        include_archived: true,
        include_following: true,
        ..Default::default()
    };
    let json = flightydeck::render::to_json(&flights::list(&fx.ctx(), &a).unwrap());
    assert!(!json.contains("eyJ") && !json.contains("test.user.token"));
    assert!(json.contains("\"flight_code\": \"QR111\""));
}
