mod common;

use common::{FRIEND, fixture};
use flightydeck::Error;
use flightydeck::model::Relation;
use flightydeck::ops::friends::{self, FriendArgs};
use flightydeck::render::Render;

#[test]
fn friends_flights_only() {
    let fx = fixture();
    let list = friends::list(&fx.ctx(), &FriendArgs::default()).unwrap();
    assert_eq!(list.count, 1);
    let f = &list.flights[0];
    assert_eq!(f.id, "f0000000-0000-4000-8000-000000000008");
    assert_eq!(f.relation, Relation::Friend);
    assert_eq!(f.friend_name.as_deref(), Some("Alex Example"));
    assert!(f.ticket.is_none());
    assert!(list.table().contains("(friend: Alex Example)"));
    assert_ne!(list.owner_user_id, FRIEND);
}

#[test]
fn name_filter_is_accent_insensitive() {
    let fx = fixture();
    let ctx = fx.ctx();
    let named = |n: &str| {
        friends::list(
            &ctx,
            &FriendArgs {
                friend: Some(n.into()),
                ..Default::default()
            },
        )
    };
    assert_eq!(named("ÁLEX").unwrap().count, 1);
    assert!(matches!(named("Nobody"), Err(Error::NotFound(_))));
    let upcoming = friends::list(
        &ctx,
        &FriendArgs {
            upcoming: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(upcoming.count, 1);
}
