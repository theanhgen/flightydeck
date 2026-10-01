mod common;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use flightydeck::{Ctx, Error, creds};

const FIXTURE_SYNC_URL: &str = "https://api.flightyapp.com/v1/sync/full?cursor=test";

fn jwt(exp: i64) -> String {
    let payload = format!(r#"{{"sub":"{}","exp":{exp}}}"#, common::OWNER);
    format!("e30.{}.c2ln", URL_SAFE_NO_PAD.encode(payload))
}

fn write_plist(ctx: &mut Ctx, dir: &std::path::Path, with_build_token: bool) {
    let mut d = plist::Dictionary::new();
    if with_build_token {
        d.insert("FlightyBuildToken".into(), "fake-build-token".into());
    }
    d.insert("CFBundleShortVersionString".into(), "9.9.9".into());
    d.insert("CFBundleVersion".into(), "999".into());
    ctx.app_plist = dir.join("Info.plist");
    plist::Value::Dictionary(d)
        .to_file_xml(&ctx.app_plist)
        .unwrap();
}

fn set_token(ctx: &Ctx, token: &str) {
    rusqlite::Connection::open(&ctx.db_path)
        .unwrap()
        .execute(
            "UPDATE Account SET authToken = ?1 WHERE rawKind = 'main'",
            [token],
        )
        .unwrap();
}

#[test]
fn loads_token_build_token_and_user_agent() {
    let f = common::fixture();
    let mut ctx = f.ctx();
    write_plist(&mut ctx, f.dir.path(), true);
    let c = creds::load(&ctx).unwrap();
    assert_eq!(c.build_token, "fake-build-token");
    assert_eq!(c.user_agent, "Flighty 9.9.9 (999) com.flightyapp.flighty");
    assert_eq!(creds::jwt_sub(&c.token).as_deref(), Some(common::OWNER));
}

#[test]
fn token_never_printed() {
    let f = common::fixture();
    let mut ctx = f.ctx();
    write_plist(&mut ctx, f.dir.path(), true);
    let token = jwt(4102444800);
    set_token(&ctx, &token);
    let c = creds::load(&ctx).unwrap();
    assert_eq!(c.token.expose(), token);
    for s in [
        format!("{c:?}"),
        format!("{}", c.token),
        format!("{:?}", c.token),
    ] {
        assert!(!s.contains(&token), "{s}");
    }
    assert!(
        !serde_json::to_string(&creds::status(&ctx))
            .unwrap()
            .contains(&token)
    );

    set_token(&ctx, &jwt(1000));
    let e = creds::load(&ctx).unwrap_err();
    assert!(!format!("{e} {e:?}").contains("e30."));
}

#[test]
fn expired_token_is_not_ready() {
    let f = common::fixture();
    let mut ctx = f.ctx();
    write_plist(&mut ctx, f.dir.path(), true);
    set_token(&ctx, &jwt(1000));
    let e = creds::load(&ctx).unwrap_err();
    assert!(matches!(e, Error::NotReady(_)), "{e}");
    assert_eq!(
        e.to_string(),
        "Flighty sign-in expired; open the Flighty app to refresh it"
    );
    assert_eq!(e.exit_code(), 3);
    assert_eq!(creds::status(&ctx).token_expired, Some(true));
}

#[test]
fn missing_pieces_are_not_ready() {
    let f = common::fixture();
    let mut ctx = f.ctx();
    // Ctx::with_db points at a nonexistent Info.plist.
    assert!(matches!(creds::load(&ctx), Err(Error::NotReady(_))));

    write_plist(&mut ctx, f.dir.path(), false);
    let e = creds::load(&ctx).unwrap_err();
    assert!(
        matches!(e, Error::NotReady(ref m) if m.contains("FlightyBuildToken")),
        "{e}"
    );

    write_plist(&mut ctx, f.dir.path(), true);
    set_token(&ctx, "");
    let e = creds::load(&ctx).unwrap_err();
    assert!(
        matches!(e, Error::NotReady(ref m) if m.contains("not signed in")),
        "{e}"
    );
}

#[test]
fn legacy_token_fallback() {
    let f = common::fixture();
    let mut ctx = f.ctx();
    write_plist(&mut ctx, f.dir.path(), true);
    set_token(&ctx, "");
    let legacy = rusqlite::Connection::open(&ctx.legacy_db_path).unwrap();
    legacy
        .execute_batch("CREATE TABLE ZUSER (Z_PK INTEGER PRIMARY KEY, ZTOKEN VARCHAR);")
        .unwrap();
    legacy
        .execute("INSERT INTO ZUSER (ZTOKEN) VALUES (?1)", [jwt(4102444800)])
        .unwrap();
    drop(legacy);
    assert_eq!(creds::load(&ctx).unwrap().token.expose(), jwt(4102444800));
}

#[test]
fn status_is_presence_only() {
    let f = common::fixture();
    let mut ctx = f.ctx();
    let s = creds::status(&ctx);
    assert!(s.token_found);
    assert_eq!(s.token_expired, Some(false));
    assert!(!s.build_token_found);
    assert_eq!(s.app_version, None);

    write_plist(&mut ctx, f.dir.path(), true);
    let s = creds::status(&ctx);
    assert!(s.build_token_found);
    assert_eq!(s.app_version.as_deref(), Some("9.9.9 (999)"));

    let missing = Ctx::with_db(f.dir.path().join("nope.db"));
    let s = creds::status(&missing);
    assert!(!s.token_found && s.token_expired.is_none());
}

#[test]
fn sync_url_from_db() {
    let f = common::fixture();
    assert_eq!(creds::sync_url(&f.ctx()).unwrap(), FIXTURE_SYNC_URL);
}
