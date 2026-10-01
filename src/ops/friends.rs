//! Connected friends' flights.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::ctx::Ctx;
use crate::db;
use crate::error::{Error, Result};
use crate::ops::flights::{self, Filter, FlightList, Scope};
use crate::owner;
use crate::time;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, clap::Args)]
pub struct FriendArgs {
    /// Friend name (accent-insensitive substring). Default: all friends.
    pub friend: Option<String>,
    /// Only upcoming flights.
    #[arg(long)]
    #[serde(default)]
    pub upcoming: bool,
    /// Maximum results (default 50).
    #[arg(long)]
    pub limit: Option<u32>,
}

pub fn list(ctx: &Ctx, a: &FriendArgs) -> Result<FlightList> {
    let conn = db::open(ctx)?;
    let owner = owner::resolve(ctx, &conn)?;
    let owner = owner.user_id.as_str();
    let name = a
        .friend
        .as_deref()
        .map(db::fold)
        .filter(|n| !n.trim().is_empty());
    let now = time::now_unix();

    let all = flights::load(&conn, owner, Scope::Friends, &Filter::default())?;
    let known_friend = name.as_ref().is_none_or(|n| {
        all.iter().any(|f| {
            f.friend_name
                .as_deref()
                .is_some_and(|x| db::fold(x).contains(n.as_str()))
        }) || friend_names(&conn, owner)
            .iter()
            .any(|x| db::fold(x).contains(n.as_str()))
    });
    if !known_friend {
        return Err(Error::NotFound(format!(
            "no connected friend matches '{}'",
            a.friend.as_deref().unwrap_or_default().trim()
        )));
    }

    let flights = all
        .into_iter()
        .filter(|f| {
            name.as_ref().is_none_or(|n| {
                f.friend_name
                    .as_deref()
                    .is_some_and(|x| db::fold(x).contains(n.as_str()))
            })
        })
        .filter(|f| !a.upcoming || flights::departure_time(f).is_some_and(|t| t >= now))
        .collect();
    Ok(flights::flight_list(
        owner,
        flights::apply_limit(flights, a.limit, a.upcoming),
    ))
}

/// Display names of all connected friends, including those with no flights.
fn friend_names(conn: &rusqlite::Connection, owner: &str) -> Vec<String> {
    let sql = "SELECT COALESCE( \
                 (SELECT p.fullName FROM Profile p WHERE p.userId = x.id LIMIT 1), x.first) \
               FROM (SELECT CASE WHEN senderUserId = ?1 THEN receiverUserId ELSE senderUserId END AS id, \
                            CASE WHEN senderUserId = ?1 THEN receiverFirstName ELSE senderFirstName END AS first \
                     FROM ConnectedFriendRelationship \
                     WHERE (senderUserId = ?1 OR receiverUserId = ?1) AND deleted IS NULL) x";
    let Ok(mut stmt) = conn.prepare(sql) else {
        return Vec::new();
    };
    stmt.query_map([owner], |r| r.get::<_, Option<String>>(0))
        .map(|rows| rows.filter_map(|r| r.ok().flatten()).collect())
        .unwrap_or_default()
}
