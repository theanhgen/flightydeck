mod common;

use common::fixture;
use flightydeck::Error;
use flightydeck::ops::flights::{self, GetArgs};
use flightydeck::ops::ics::{self, IcsArgs};

/// 2031-01-01 00:00:00 UTC.
const NOW: i64 = 1_924_992_000;

fn unfold(ics: &str) -> String {
    ics.replace("\r\n ", "")
}

/// The unfolded lines of the event with this UID.
fn event(ics: &str, id: &str) -> Vec<String> {
    let text = unfold(ics);
    let lines: Vec<&str> = text.split("\r\n").collect();
    let at = lines
        .iter()
        .position(|l| *l == format!("UID:{id}@flightydeck"))
        .unwrap_or_else(|| panic!("no event {id}"));
    let end = at + lines[at..].iter().position(|l| *l == "END:VEVENT").unwrap();
    lines[at - 1..=end].iter().map(|l| l.to_string()).collect()
}

#[test]
fn one_flight_is_one_complete_event() {
    let fx = fixture();
    let f = flights::get(
        &fx.ctx(),
        &GetArgs {
            flight: "QR111".into(),
            date: Some("2031-03-17".into()),
        },
    )
    .unwrap();
    let cal = ics::calendar(std::slice::from_ref(&f), NOW);
    assert_eq!((cal.count, cal.skipped), (1, 0));
    assert_eq!(
        unfold(&cal.ics),
        [
            "BEGIN:VCALENDAR",
            "VERSION:2.0",
            "PRODID:-//flightydeck//EN",
            "CALSCALE:GREGORIAN",
            "X-WR-CALNAME:Flights",
            "BEGIN:VEVENT",
            "UID:f0000000-0000-4000-8000-000000000001@flightydeck",
            "DTSTAMP:20310101T000000Z",
            "DTSTART:20310317T145000Z",
            "DTEND:20310317T204000Z",
            "SUMMARY:QR111 PRG → DOH",
            "LOCATION:Václav Havel Prague (PRG)\\, Terminal 1",
            "DESCRIPTION:Qatar Airways QR111\\nDeparts: Prague (PRG)\\, Mon 17 Mar 2031 15:50 +01\\, \
             terminal 1\\, gate A1\\nArrives: Doha (DOH)\\, Mon 17 Mar 2031 23:40 +03\\nSeat: 32A\\n\
             Cabin: economy\\nBooking: TEST01\\nAircraft: Airbus A350-900 (A7-TST)",
            "END:VEVENT",
            "END:VCALENDAR",
            "",
        ]
        .join("\r\n")
    );
}

#[test]
fn every_line_is_crlf_and_at_most_75_octets() {
    let fx = fixture();
    let cal = ics::ics(&fx.ctx(), &IcsArgs::default()).unwrap();
    assert!(cal.ics.ends_with("END:VCALENDAR\r\n"));
    assert!(!cal.ics.replace("\r\n", "").contains(['\r', '\n']));
    for line in cal.ics.split("\r\n") {
        assert!(line.len() <= 75, "{line}");
    }
}

#[test]
fn filters_match_list_and_uids_are_stable() {
    let fx = fixture();
    let ctx = fx.ctx();
    let all = ics::ics(&ctx, &IcsArgs::default()).unwrap();
    assert_eq!((all.count, all.skipped), (8, 0));
    assert_eq!(all.ics.matches("BEGIN:VEVENT").count(), 8);

    let upcoming = IcsArgs {
        upcoming: true,
        ..Default::default()
    };
    let a = ics::ics(&ctx, &upcoming).unwrap();
    let b = ics::ics(&ctx, &upcoming).unwrap();
    assert_eq!(a.count, 5);
    let uids = |c: &ics::Calendar| -> Vec<String> {
        c.ics
            .split("\r\n")
            .filter(|l| l.starts_with("UID:"))
            .map(str::to_string)
            .collect()
    };
    assert_eq!(uids(&a), uids(&b));

    let y2025 = ics::ics(
        &ctx,
        &IcsArgs {
            year: Some(2025),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(y2025.count, 2);
}

#[test]
fn cancelled_flight_is_marked() {
    let fx = fixture();
    let cal = ics::ics(&fx.ctx(), &IcsArgs::default()).unwrap();
    let e = event(&cal.ics, "f0000000-0000-4000-8000-000000000009");
    assert!(e.contains(&"STATUS:CANCELLED".to_string()), "{e:?}");
    assert!(
        e.iter()
            .any(|l| l.starts_with("SUMMARY:Cancelled: ") && l.contains("7777")),
        "{e:?}"
    );
}

#[test]
fn manual_flight_gets_times_and_no_ticket_lines() {
    let fx = fixture();
    let cal = ics::ics(&fx.ctx(), &IcsArgs::default()).unwrap();
    let e = event(&cal.ics, "e0000000-0000-4000-8000-000000000001");
    assert!(e.contains(&"DTSTART:20250301T060000Z".to_string()), "{e:?}");
    assert!(e.contains(&"DTEND:20250301T071500Z".to_string()), "{e:?}");
    assert!(!e.iter().any(|l| l.contains("Seat:")), "{e:?}");
}

#[test]
fn unknown_flight_is_not_found() {
    let fx = fixture();
    let e = ics::ics(
        &fx.ctx(),
        &IcsArgs {
            flight: Some("ZZ999".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(e, Error::NotFound(_)), "{e}");
}
