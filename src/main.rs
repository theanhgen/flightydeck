//! `flightydeck` CLI. Parses args, calls `flightydeck::ops`, renders, maps errors to exit codes.

use std::io::{BufRead, IsTerminal, Write};

use clap::{Parser, Subcommand};
use flightydeck::ctx::{Class, Ctx};
use flightydeck::ops::{
    about, connections, connections::ConnArgs, flights, flights::CurrentArgs, flights::GetArgs,
    flights::ListArgs, flights::SearchArgs, friends, friends::FriendArgs, reference,
    reference::LookupArgs, stats, stats::StatsArgs, status, status::FlightRef, write,
    write::AddArgs, write::RemoveArgs,
};
use flightydeck::render::{self, Format};
use flightydeck::{Error, Result};

/// Unofficial CLI for your Flighty flights (macOS). Not affiliated with Flighty.
#[derive(Debug, Parser)]
#[command(
    name = "flightydeck",
    version,
    long_about = "Unofficial CLI for your Flighty flights (macOS). Not affiliated with Flighty.\n\n\
        Reads come from the Flighty app's local database; add/follow/remove go through Flighty's API.\n\
        FLIGHTY_READ_ONLY=1 disables every write. FLIGHTY_ALLOW_REMOVE=1 enables the experimental remove.\n\n\
        Exit codes: 0 ok, 1 error, 2 bad input, 3 not signed in / no database, 4 Flighty API error,\n\
        5 refused by policy, 6 not found."
)]
struct Cli {
    /// Output format: a table for humans, or JSON (same shape as the MCP tool output).
    #[arg(short, long, value_enum, default_value_t = Format::Table, global = true)]
    output: Format,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// List your flights.
    List(ListArgs),
    /// Show one flight by code or UUID.
    #[command(long_about = "Show one flight by code or Flighty UUID.\n\n\
        Examples:\n  flightydeck get QR111\n  flightydeck get \"VN 333\" --date 2026-10-14\n  \
        flightydeck -o json get 3f2c9a1e-0000-4000-8000-000000000000")]
    Get(GetArgs),
    /// Search your flights by text, airports, airline or dates.
    Search(SearchArgs),
    /// Flights in the air now, departing soon, or just landed.
    Current(CurrentArgs),
    /// Status of one flight: phase, delays, gates, times.
    Status(FlightRef),
    /// Delay forecast for a flight code from its local history.
    #[command(visible_alias = "forecast")]
    Delay(FlightRef),
    /// Flights of your connected Flighty friends.
    Friends(FriendArgs),
    /// Statistics of your own flights, lifetime or per year.
    Stats(StatsArgs),
    /// Connections (layovers) between your flights.
    Connections(ConnArgs),
    /// Find airports by name, city, IATA or ICAO code.
    Airports(LookupArgs),
    /// Find airlines by name, IATA or ICAO code.
    Airlines(LookupArgs),
    /// Add a flight you are flying to your Flighty account.
    #[command(
        long_about = "Add a flight you are flying to your Flighty account.\n\n\
        The date is the local departure date at the origin airport. Refused in read-only mode \
        (FLIGHTY_READ_ONLY=1). If the flight already looks tracked, nothing is sent unless --force.\n\n\
        Examples:\n  flightydeck add VN333 2026-10-14\n  flightydeck add \"QR 111\" 2026-11-02 --force"
    )]
    Add(AddArgs),
    /// Follow a flight you are not on.
    Follow(AddArgs),
    /// EXPERIMENTAL: remove a flight from your Flighty account.
    #[command(
        long_about = "EXPERIMENTAL: remove a flight from your Flighty account. Cannot be undone.\n\n\
        Needs FLIGHTY_ALLOW_REMOVE=1 and is refused in read-only mode. Asks for confirmation on a \
        terminal; when stdin is not a terminal, --yes is required.\n\n\
        Examples:\n  FLIGHTY_ALLOW_REMOVE=1 flightydeck remove 3f2c9a1e-0000-4000-8000-000000000000\n  \
        FLIGHTY_ALLOW_REMOVE=1 flightydeck remove 3f2c9a1e-0000-4000-8000-000000000000 --yes"
    )]
    Remove(RemoveArgs),
    /// Version, mode, and whether the Flighty database and sign-in were found.
    About,
    /// Run the MCP server on stdio (same operations as the CLI).
    Mcp,
}

fn main() {
    let cli = Cli::parse();
    let fmt = cli.output;
    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        if fmt == Format::Json {
            println!("{}", render::to_json(&flightydeck::mcp::error_json(&e)));
        }
        std::process::exit(e.exit_code());
    }
}

fn run(cli: Cli) -> Result<()> {
    let ctx = Ctx::from_env();
    let f = cli.output;
    match cli.cmd {
        Cmd::List(a) => render::print(&flights::list(&ctx, &a)?, f),
        Cmd::Get(a) => render::print(&flights::get(&ctx, &a)?, f),
        Cmd::Search(a) => render::print(&flights::search(&ctx, &a)?, f),
        Cmd::Current(a) => render::print(&flights::current(&ctx, &a)?, f),
        Cmd::Status(a) => render::print(&status::flight_status(&ctx, &a)?, f),
        Cmd::Delay(a) => render::print(&status::delay_forecast(&ctx, &a)?, f),
        Cmd::Friends(a) => render::print(&friends::list(&ctx, &a)?, f),
        Cmd::Stats(a) => render::print(&stats::stats(&ctx, &a)?, f),
        Cmd::Connections(a) => render::print(&connections::list(&ctx, &a)?, f),
        Cmd::Airports(a) => render::print(&reference::airports(&ctx, &a)?, f),
        Cmd::Airlines(a) => render::print(&reference::airlines(&ctx, &a)?, f),
        Cmd::Add(a) => render::print(&write::add(&ctx, &a)?, f),
        Cmd::Follow(a) => render::print(&write::follow(&ctx, &a)?, f),
        Cmd::Remove(mut a) => {
            // Policy first, so nobody is asked to confirm something that would be refused anyway.
            ctx.guard(Class::Destructive)?;
            confirm_remove(&mut a)?;
            render::print(&write::remove(&ctx, &a)?, f)
        }
        Cmd::About => render::print(&about::about(&ctx)?, f),
        Cmd::Mcp => serve_mcp()?,
    }
    Ok(())
}

/// Asks y/N on stderr when stdin is a terminal; without a terminal, only `--yes` confirms.
fn confirm_remove(a: &mut RemoveArgs) -> Result<()> {
    if a.yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        return Err(Error::Refused(
            "refusing to remove without --yes (stdin is not a terminal)".into(),
        ));
    }
    eprint!(
        "Remove flight {} from your Flighty account? This cannot be undone. [y/N] ",
        a.id
    );
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|e| Error::Other(format!("reading confirmation: {e}")))?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        a.yes = true;
        Ok(())
    } else {
        Err(Error::Refused("remove cancelled".into()))
    }
}

fn serve_mcp() -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Other(format!("starting the async runtime: {e}")))?
        .block_on(flightydeck::mcp::serve())
}
