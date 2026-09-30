mod common;

use common::{OWNER, fixture};
use flightydeck::{Error, db, owner};

#[test]
fn env_override_wins() {
    let fx = fixture();
    let mut ctx = fx.ctx();
    ctx.owner_override = Some("someone-else".into());
    let o = owner::resolve(&ctx, &db::open(&ctx).unwrap()).unwrap();
    assert_eq!((o.user_id.as_str(), o.source), ("someone-else", "env"));
}

#[test]
fn main_account_user_row() {
    let fx = fixture();
    let ctx = fx.ctx();
    let o = owner::resolve(&ctx, &db::open(&ctx).unwrap()).unwrap();
    assert_eq!((o.user_id.as_str(), o.source), (OWNER, "account"));
}

#[test]
fn jwt_sub_when_user_row_missing() {
    let fx = fixture();
    rusqlite::Connection::open(&fx.db)
        .unwrap()
        .execute_batch("DELETE FROM User;")
        .unwrap();
    let ctx = fx.ctx();
    let o = owner::resolve(&ctx, &db::open(&ctx).unwrap()).unwrap();
    assert_eq!((o.user_id.as_str(), o.source), (OWNER, "jwt"));
}

#[test]
fn unresolved_is_not_ready_and_hides_token() {
    let fx = fixture();
    rusqlite::Connection::open(&fx.db)
        .unwrap()
        .execute_batch("DELETE FROM User; UPDATE Account SET authToken = 'secret-token-value';")
        .unwrap();
    let ctx = fx.ctx();
    let err = owner::resolve(&ctx, &db::open(&ctx).unwrap()).unwrap_err();
    assert!(matches!(err, Error::NotReady(_)));
    assert!(err.to_string().contains("FLIGHTY_OWNER_USER_ID"));
    assert!(!format!("{err} {err:?}").contains("secret-token-value"));
}
