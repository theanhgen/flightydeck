//! Which Flighty userId is "me". Tiers (no frequency guessing):
//! 1. FLIGHTY_OWNER_USER_ID  2. User.remoteId for the main Account  3. JWT `sub`  4. error.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rusqlite::{Connection, OptionalExtension};
use zeroize::Zeroizing;

use crate::ctx::Ctx;
use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Owner {
    pub user_id: String,
    /// "env" | "account" | "jwt"
    pub source: &'static str,
}

pub fn resolve(ctx: &Ctx, conn: &Connection) -> Result<Owner> {
    if let Some(id) = &ctx.owner_override {
        return Ok(Owner {
            user_id: id.clone(),
            source: "env",
        });
    }

    // One read transaction so the Account and User rows come from the same snapshot.
    let tx = conn.unchecked_transaction()?;
    let account: Option<(i64, Zeroizing<String>)> = tx
        .query_row(
            "SELECT id, authToken FROM Account WHERE rawKind = 'main' ORDER BY id LIMIT 1",
            [],
            |r| Ok((r.get(0)?, Zeroizing::new(r.get::<_, String>(1)?))),
        )
        .optional()?;
    let Some((account_id, token)) = account else {
        return Err(unresolved());
    };

    // A User table problem must not stop the JWT tier, so its errors fall through.
    let remote_id: Option<String> = tx
        .query_row(
            "SELECT remoteId FROM User WHERE accountId = ?1",
            [account_id],
            |r| r.get(0),
        )
        .ok()
        .filter(|s: &String| !s.trim().is_empty());
    if let Some(id) = remote_id {
        return Ok(Owner {
            user_id: id,
            source: "account",
        });
    }

    jwt_sub(&token)
        .map(|user_id| Owner {
            user_id,
            source: "jwt",
        })
        .ok_or_else(unresolved)
}

fn unresolved() -> Error {
    Error::NotReady("owner unresolved; set FLIGHTY_OWNER_USER_ID".into())
}

/// `sub` claim of a JWT. Decodes the payload only; the signature is not checked (we only need
/// to know whose token the app stored). Errors are swallowed so nothing token-derived leaks.
fn jwt_sub(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?);
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims
        .get("sub")?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::jwt_sub;

    #[test]
    fn reads_sub_without_signature_check() {
        // {"sub":"u-1"} with and without padding, garbage signature.
        assert_eq!(jwt_sub("e30.eyJzdWIiOiJ1LTEifQ.x").as_deref(), Some("u-1"));
        assert_eq!(
            jwt_sub("e30.eyJzdWIiOiJ1LTEifQ==.x").as_deref(),
            Some("u-1")
        );
        assert_eq!(jwt_sub("not-a-jwt"), None);
        assert_eq!(jwt_sub("e30.e30.x"), None);
    }
}
