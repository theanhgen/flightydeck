//! `flightydeck` CLI. Parses args, calls `flightydeck::ops`, renders, maps errors to exit codes.

use std::io::{BufRead, IsTerminal, Write};

use clap::{Parser, Subcommand};
use flightydeck::ctx::{Class, Ctx};
use flightydeck::ops::{
    about, connections, connections::ConnArgs, flights, flights::CurrentArgs, flights::GetArgs,
    flights::ListArgs, flights::SearchArgs, friends, friends::FriendArgs, ics, ics::IcsArgs,
    reference, reference::LookupArgs, stats, stats::StatsArgs, status, status::FlightRef, watch,
    watch::WatchArgs, write, write::AddArgs, write::RemoveArgs,
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
        Examples:\n  flightydeck get BA286\n  flightydeck get \"UA 901\" --date 2031-03-14\n  \
        flightydeck -o json get 3f2c9a1e-0000-4000-8000-000000000000")]
    Get(GetArgs),
    /// Search your flights by text, airports, airline or dates.
    Search(SearchArgs),
    /// Flights in the air now, departing soon, or just landed.
    Current(CurrentArgs),
    /// Status of one flight: phase, delays, gates, times.
    Status(FlightRef),
    /// Follow one flight: print its status again on an interval.
    #[command(
        long_about = "Follow one flight: print its status again on an interval, with what changed \
        (gate, times, delays). Stops when the flight has landed or was cancelled; Ctrl-C stops it sooner.\n\n\
        Reads the local database only, so it sees what the Flighty app has synced: keep the app \
        running. While the flight is in the air the position is an ESTIMATE along the direct route, \
        worked out from the departure and arrival times. It is not live tracking, and there is no \
        altitude. With -o json each update is one line of JSON.\n\n\
        Examples:\n  flightydeck watch BA286\n  flightydeck watch BA286 --every 1m\n  \
        flightydeck watch \"UA 901\" --date 2031-03-14 --every 1h\n  flightydeck -o json watch BA286 --once"
    )]
    Watch(WatchArgs),
    /// Delay forecast for a flight code from its local history.
    #[command(visible_alias = "forecast")]
    Delay(FlightRef),
    /// Your flights as an iCalendar (.ics) file, for any calendar app.
    #[command(
        long_about = "Print your flights as an iCalendar (.ics) file, for any calendar app.\n\n\
        Reads the local database only. Each event's UID is the Flighty flight id, so importing a \
        newer export updates events in calendar apps that match on UID. Times are UTC; the calendar \
        app shows them in your zone. With -o json the file is in the `ics` field.\n\n\
        Examples:\n  flightydeck ics --upcoming > flights.ics\n  flightydeck ics --year 2031 > 2031.ics\n  \
        flightydeck ics BA286 --date 2031-03-17 > ba286.ics"
    )]
    Ics(IcsArgs),
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
        Examples:\n  flightydeck add UA901 2031-03-14\n  flightydeck add \"BA 286\" 2031-04-02 --force"
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
        Cmd::Watch(a) => run_watch(&ctx, &a, f)?,
        Cmd::Delay(a) => render::print(&status::delay_forecast(&ctx, &a)?, f),
        // The file already ends each line itself, so it is printed as is.
        Cmd::Ics(a) => match f {
            Format::Table => print!("{}", ics::ics(&ctx, &a)?.ics),
            Format::Json => render::print(&ics::ics(&ctx, &a)?, f),
        },
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

/// Prints a reading now and after every interval, until the flight is done or `--once`.
fn run_watch(ctx: &Ctx, a: &WatchArgs, fmt: Format) -> Result<()> {
    let every = watch::parse_interval(&a.every)?;
    let mut target = FlightRef {
        flight: a.flight.clone(),
        date: a.date.clone(),
    };
    let mut previous: Option<watch::Snapshot> = None;
    loop {
        let snap = watch::snapshot(ctx, &target, previous.as_ref())?;
        let mut out = std::io::stdout().lock();
        let written = match fmt {
            Format::Json => serde_json::to_string(&snap)
                .map_err(|e| Error::Other(format!("serialize: {e}")))
                .map(|line| writeln!(out, "{line}")),
            Format::Table => {
                let at = chrono::Local::now().format("%H:%M");
                let mut text = format!("{at}  {}", snap.line());
                for change in &snap.changes {
                    text.push_str(&format!("\n       changed: {change}"));
                }
                Ok(writeln!(out, "{text}"))
            }
        }?;
        // A closed pipe (`| head`) ends the watch quietly.
        if written.and_then(|()| out.flush()).is_err() || a.once || snap.done {
            return Ok(());
        }
        drop(out);
        // From now on follow this exact flight, not "the nearest one with this code".
        target = FlightRef {
            flight: snap.flight_id.clone(),
            date: None,
        };
        previous = Some(snap);
        std::thread::sleep(every);
    }
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
