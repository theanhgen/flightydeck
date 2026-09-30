//! `flighty mcp`: stdio MCP server. Each tool = one `ops::*` call. stdout is JSON-RPC only.

use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use serde::Serialize;

use crate::ctx::{Class, Ctx};
use crate::error::Error;
use crate::ops::{
    about, connections, connections::ConnArgs, flights, flights::CurrentArgs, flights::GetArgs,
    flights::ListArgs, flights::SearchArgs, friends, friends::FriendArgs, reference,
    reference::LookupArgs, stats, stats::StatsArgs, status, status::FlightRef, write,
    write::AddArgs, write::RemoveArgs,
};

const INSTRUCTIONS: &str = "Unofficial access to the user's Flighty flights (not affiliated with Flighty). \
Reads come from the local Flighty database. Write tools (add/follow) exist only when the server is not in \
read-only mode (FLIGHTY_READ_ONLY=1); remove exists only with FLIGHTY_ALLOW_REMOVE=1 and needs yes=true. \
Times are given as utc, local and tz.";

/// Tools that change the account. Registered only if `ctx.allows(Class::Write)`.
const WRITE_TOOLS: &[&str] = &["flighty_add_flight", "flighty_follow_flight"];
/// Registered only if `ctx.allows(Class::Destructive)`.
const DESTRUCTIVE_TOOLS: &[&str] = &["flighty_remove_flight"];

/// The same `{"error":{"kind","message"}}` body the CLI prints in `-o json` mode.
pub fn error_json(e: &Error) -> serde_json::Value {
    serde_json::json!({ "error": { "kind": e.kind(), "message": e.to_string() } })
}

#[derive(Debug, Clone)]
pub struct FlightyServer {
    ctx: Ctx,
    tool_router: ToolRouter<Self>,
}

impl FlightyServer {
    /// Tool registration follows the policy at startup; the ops still guard every call.
    pub fn new(ctx: Ctx) -> Self {
        let mut tool_router = Self::tool_router();
        let mut drop_tools =
            |names: &[&str]| names.iter().for_each(|n| tool_router.remove_route(n));
        if !ctx.allows(Class::Write) {
            drop_tools(WRITE_TOOLS);
        }
        if !ctx.allows(Class::Destructive) {
            drop_tools(DESTRUCTIVE_TOOLS);
        }
        FlightyServer { ctx, tool_router }
    }

    /// Runs a blocking op off the async runtime. Op failures become tool errors (visible to the
    /// caller), never protocol errors.
    async fn run<T, F>(&self, op: F) -> ToolResult
    where
        T: Serialize + Send + 'static,
        F: FnOnce(&Ctx) -> crate::Result<T> + Send + 'static,
    {
        let ctx = self.ctx.clone();
        let outcome = tokio::task::spawn_blocking(move || op(&ctx))
            .await
            .unwrap_or_else(|_| {
                Err(Error::Other(
                    "internal error: the operation panicked".into(),
                ))
            });
        Ok(match outcome {
            Ok(v) => CallToolResult::success(vec![ContentBlock::text(crate::render::to_json(&v))]),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(crate::render::to_json(
                &error_json(&e),
            ))]),
        })
    }
}

type ToolResult = Result<CallToolResult, ErrorData>;

#[tool_router]
impl FlightyServer {
    #[tool(
        name = "flighty_list_flights",
        description = "List the owner's flights (newest first). Filters: upcoming, past, include_archived, include_following, limit.",
        annotations(read_only_hint = true)
    )]
    async fn list_flights(&self, Parameters(a): Parameters<ListArgs>) -> ToolResult {
        self.run(move |c| flights::list(c, &a)).await
    }

    #[tool(
        name = "flighty_get_flight",
        description = "One flight by code (e.g. \"QR111\") or Flighty UUID; optional local departure date YYYY-MM-DD.",
        annotations(read_only_hint = true)
    )]
    async fn get_flight(&self, Parameters(a): Parameters<GetArgs>) -> ToolResult {
        self.run(move |c| flights::get(c, &a)).await
    }

    #[tool(
        name = "flighty_search_flights",
        description = "Search the owner's flights by free text, from/to airport, airline and date range.",
        annotations(read_only_hint = true)
    )]
    async fn search_flights(&self, Parameters(a): Parameters<SearchArgs>) -> ToolResult {
        self.run(move |c| flights::search(c, &a)).await
    }

    #[tool(
        name = "flighty_current_flights",
        description = "Flights in the air now, departing soon, or just landed.",
        annotations(read_only_hint = true)
    )]
    async fn current_flights(&self, Parameters(a): Parameters<CurrentArgs>) -> ToolResult {
        self.run(move |c| flights::current(c, &a)).await
    }

    #[tool(
        name = "flighty_get_flight_status",
        description = "Status of one flight from the local database: phase, delays, gates, times.",
        annotations(read_only_hint = true)
    )]
    async fn get_flight_status(&self, Parameters(a): Parameters<FlightRef>) -> ToolResult {
        self.run(move |c| status::flight_status(c, &a)).await
    }

    #[tool(
        name = "flighty_get_delay_forecast",
        description = "Delay forecast for a flight code from past flights of the same code stored locally.",
        annotations(read_only_hint = true)
    )]
    async fn get_delay_forecast(&self, Parameters(a): Parameters<FlightRef>) -> ToolResult {
        self.run(move |c| status::delay_forecast(c, &a)).await
    }

    #[tool(
        name = "flighty_list_friend_flights",
        description = "Flights of connected Flighty friends, optionally filtered by friend name.",
        annotations(read_only_hint = true)
    )]
    async fn list_friend_flights(&self, Parameters(a): Parameters<FriendArgs>) -> ToolResult {
        self.run(move |c| friends::list(c, &a)).await
    }

    #[tool(
        name = "flighty_get_flight_stats",
        description = "Lifetime or per-year statistics of the owner's own flights.",
        annotations(read_only_hint = true)
    )]
    async fn get_flight_stats(&self, Parameters(a): Parameters<StatsArgs>) -> ToolResult {
        self.run(move |c| stats::stats(c, &a)).await
    }

    #[tool(
        name = "flighty_get_connections",
        description = "Connections (layovers) between the owner's flights, with layover time and risk.",
        annotations(read_only_hint = true)
    )]
    async fn get_connections(&self, Parameters(a): Parameters<ConnArgs>) -> ToolResult {
        self.run(move |c| connections::list(c, &a)).await
    }

    #[tool(
        name = "flighty_search_airports",
        description = "Find airports by name, city, IATA or ICAO code (accent-insensitive).",
        annotations(read_only_hint = true)
    )]
    async fn search_airports(&self, Parameters(a): Parameters<LookupArgs>) -> ToolResult {
        self.run(move |c| reference::airports(c, &a)).await
    }

    #[tool(
        name = "flighty_search_airlines",
        description = "Find airlines by name, IATA or ICAO code.",
        annotations(read_only_hint = true)
    )]
    async fn search_airlines(&self, Parameters(a): Parameters<LookupArgs>) -> ToolResult {
        self.run(move |c| reference::airlines(c, &a)).await
    }

    #[tool(
        name = "flighty_about",
        description = "Version, mode (read-only or read-write), and whether the Flighty database and sign-in were found.",
        annotations(read_only_hint = true)
    )]
    async fn about(&self) -> ToolResult {
        self.run(about::about).await
    }

    #[tool(
        name = "flighty_add_flight",
        description = "Add a flight you are flying to your Flighty account (code + local departure date YYYY-MM-DD).",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = true
        )
    )]
    async fn add_flight(&self, Parameters(a): Parameters<AddArgs>) -> ToolResult {
        self.run(move |c| write::add(c, &a)).await
    }

    #[tool(
        name = "flighty_follow_flight",
        description = "Follow a flight you are not on (code + local departure date YYYY-MM-DD).",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = true
        )
    )]
    async fn follow_flight(&self, Parameters(a): Parameters<AddArgs>) -> ToolResult {
        self.run(move |c| write::follow(c, &a)).await
    }

    #[tool(
        name = "flighty_remove_flight",
        description = "EXPERIMENTAL: remove a flight from your Flighty account by UUID. Requires yes=true. Cannot be undone.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            open_world_hint = true
        )
    )]
    async fn remove_flight(&self, Parameters(a): Parameters<RemoveArgs>) -> ToolResult {
        self.run(move |c| {
            if !a.yes {
                return Err(Error::Refused("remove needs yes=true to confirm".into()));
            }
            write::remove(c, &a)
        })
        .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for FlightyServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "flightydeck",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS.to_string())
    }
}

pub async fn serve() -> crate::Result<()> {
    let server = FlightyServer::new(Ctx::from_env());
    let running = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| Error::Other(format!("MCP initialize failed: {e}")))?;
    running
        .waiting()
        .await
        .map_err(|e| Error::Other(format!("MCP server stopped: {e}")))?;
    Ok(())
}
