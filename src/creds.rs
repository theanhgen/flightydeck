//! Credentials: auth token + sync URL from the DB, build token + app version from Info.plist.
//! The token never leaves this module and `api.rs` in any printable form.

use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::ctx::Ctx;
use crate::db;
use crate::error::{Error, Result};

/// Auth token. `Debug`/`Display` print `[redacted]`; zeroed on drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Token(String);

impl Token {
    pub fn new(s: String) -> Self {
        Token(s)
    }
    /// Only `api.rs` should call this, to build the Authorization header.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}
impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

#[derive(Debug)]
pub struct Creds {
    pub token: Token,
    pub build_token: String,
    /// "Flighty 4.11.0 (4813) com.flightyapp.flighty"
    pub user_agent: String,
}

/// Read everything needed for an API call, fresh (per call). Checks JWT `exp`.
pub fn load(ctx: &Ctx) -> Result<Creds> {
    let token = token(ctx)?;
    if jwt_expired(&token) == Some(true) {
        return Err(Error::NotReady(
            "Flighty sign-in expired; open the Flighty app to refresh it".into(),
        ));
    }
    let app = app_info(ctx)?;
    let build_token = app.build_token.ok_or_else(|| {
        Error::NotReady(format!(
            "FlightyBuildToken missing from {}; update the Flighty app",
            ctx.app_plist.display()
        ))
    })?;
    let version = app.version.ok_or_else(|| {
        Error::NotReady(format!(
            "app version missing from {}; update the Flighty app",
            ctx.app_plist.display()
        ))
    })?;
    Ok(Creds {
        token,
        build_token,
        user_agent: format!("Flighty {version} com.flightyapp.flighty"),
    })
}

/// The auth token: `Account.authToken` of the main account, else the legacy
/// `Flighty.sqlite` `ZUSER.ZTOKEN`. Does not check expiry.
pub fn token(ctx: &Ctx) -> Result<Token> {
    let conn = db::open(ctx)?;
    let main: Option<String> = conn
        .query_row(
            "SELECT authToken FROM Account WHERE rawKind = 'main' AND authToken <> '' ORDER BY id LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    drop(conn);
    if let Some(t) = main {
        return Ok(Token::new(t));
    }
    legacy_token(&ctx.legacy_db_path).ok_or_else(|| {
        Error::NotReady(
            "Flighty not signed in: no auth token found. Open the Flighty app and sign in.".into(),
        )
    })
}

/// Best effort: the legacy Core Data store is empty on current builds and may not exist.
fn legacy_token(path: &Path) -> Option<Token> {
    if !path.exists() {
        return None;
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    conn.query_row(
        "SELECT ZTOKEN FROM ZUSER WHERE ZTOKEN IS NOT NULL AND ZTOKEN <> '' LIMIT 1",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .map(Token::new)
}

/// Payload claims of a JWT (no signature check: we only read our own token).
fn jwt_claims(token: &Token) -> Option<serde_json::Value> {
    let payload = token.expose().split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// `Some(true)` if the `exp` claim is in the past, `None` if it can't be read.
pub fn jwt_expired(token: &Token) -> Option<bool> {
    let exp = jwt_claims(token)?.get("exp")?.as_i64()?;
    Some(exp <= chrono::Utc::now().timestamp())
}

/// The JWT `sub` claim (owner resolution tier 3).
pub fn jwt_sub(token: &Token) -> Option<String> {
    jwt_claims(token)?
        .get("sub")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

struct AppInfo {
    build_token: Option<String>,
    /// "4.11.0 (4813)"
    version: Option<String>,
}

fn app_info(ctx: &Ctx) -> Result<AppInfo> {
    let value = plist::Value::from_file(&ctx.app_plist).map_err(|_| {
        Error::NotReady(format!(
            "Flighty app not found (can't read {}). Install the Flighty Mac app.",
            ctx.app_plist.display()
        ))
    })?;
    let dict = value.as_dictionary();
    let get = |k: &str| {
        dict.and_then(|d| d.get(k))
            .and_then(plist::Value::as_string)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let version = match (get("CFBundleShortVersionString"), get("CFBundleVersion")) {
        (Some(short), Some(build)) => Some(format!("{short} ({build})")),
        _ => None,
    };
    Ok(AppInfo {
        build_token: get("FlightyBuildToken"),
        version,
    })
}

/// Latest `SyncInfo.nextURL`, preferring the main account (only needed for remove).
pub fn sync_url(ctx: &Ctx) -> Result<String> {
    let conn = db::open(ctx)?;
    let url: Option<String> = conn
        .query_row(
            "SELECT nextURL FROM SyncInfo WHERE nextURL IS NOT NULL AND nextURL <> ''
             ORDER BY accountId = (SELECT id FROM Account WHERE rawKind = 'main' ORDER BY id LIMIT 1) DESC,
                      lastSync DESC
             LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    url.ok_or_else(|| {
        Error::NotReady(
            "Flighty has no sync state yet; open the Flighty app and let it sync once".into(),
        )
    })
}

/// Presence only, for `about`. Never returns the token.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CredStatus {
    pub token_found: bool,
    pub token_expired: Option<bool>,
    pub build_token_found: bool,
    pub app_version: Option<String>,
}

pub fn status(ctx: &Ctx) -> CredStatus {
    let token = token(ctx).ok();
    let app = app_info(ctx).ok();
    CredStatus {
        token_found: token.is_some(),
        token_expired: token.as_ref().and_then(jwt_expired),
        build_token_found: app.as_ref().is_some_and(|a| a.build_token.is_some()),
        app_version: app.and_then(|a| a.version),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(payload: &str) -> Token {
        Token::new(format!("e30.{}.c2ln", URL_SAFE_NO_PAD.encode(payload)))
    }

    #[test]
    fn reads_exp_and_sub() {
        let t = jwt(r#"{"sub":"u1","exp":4102444800}"#);
        assert_eq!(jwt_expired(&t), Some(false));
        assert_eq!(jwt_sub(&t).as_deref(), Some("u1"));
        assert_eq!(jwt_expired(&jwt(r#"{"exp":1000}"#)), Some(true));
    }

    #[test]
    fn unreadable_jwt_is_unknown() {
        assert_eq!(jwt_expired(&Token::new("opaque".into())), None);
        assert_eq!(jwt_expired(&Token::new("a.!!!.b".into())), None);
        assert_eq!(jwt_expired(&jwt(r#"{"sub":"u1"}"#)), None);
    }

    #[test]
    fn token_is_redacted() {
        let t = Token::new("secret-value".into());
        assert_eq!(format!("{t:?} {t}"), "[redacted] [redacted]");
    }
}
