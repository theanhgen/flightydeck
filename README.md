# flightydeck

> **Unofficial.** Not made by, endorsed by, or affiliated with Flighty. "Flighty" is a trademark
> of its owner. flightydeck reads the Flighty Mac app's private database and calls the same
> private API the app uses; both can change without notice, and using them may be subject to
> Flighty's terms. Use it for your own account, at your own risk.

A command-line tool (`flightydeck`) and an MCP server (`flightydeck mcp`) for the
[Flighty](https://flightyapp.com) macOS app. It reads the flights you already track in the app
from its local database (read-only), and adds or follows new flights through the same API the
app uses.

Website: <https://theanhgen.github.io/flightydeck/>

## Install

Requirements: macOS, and the Flighty Mac app installed and signed in.

```sh
brew install theanhgen/tap/flightydeck
flightydeck about                        # checks it can find Flighty
```

Update with `brew upgrade flightydeck`; the formula follows every release.

Or build it yourself with a Rust toolchain:

```sh
cargo install --git https://github.com/theanhgen/flightydeck
```

Each [release](https://github.com/theanhgen/flightydeck/releases) also carries the universal
(Apple Silicon + Intel) binary as a tarball with its SHA-256.

It's a single static binary. No Node or Python runtime.

## Quick start

```sh
flightydeck list --upcoming              # your next flights
flightydeck status QR111                 # schedule vs estimate vs actual, gate, terminal, belt
flightydeck airports "con dao"           # accent-insensitive: finds Côn Đảo
flightydeck stats --year 2031            # distance, time in the air, top routes
flightydeck add VN333 2031-03-18         # add a flight you're on
flightydeck ics --upcoming > flights.ics  # your flights for any calendar app
flightydeck list -o json | jq '.[0]'     # JSON for scripts and agents
```

## Commands

Run `flightydeck <command> --help` for every flag.

| Command | What it does | Class |
|---|---|---|
| `list` | Your flights (`--upcoming`, `--past`, `--year`, `--include-archived`, `--include-following`, `--limit`, `--offset`) | read |
| `get` | One flight by code or Flighty UUID (`--date`), including ticket details | read |
| `search` | Filter by `--airline`, `--from`, `--to`, `--after`, `--before` | read |
| `current` | Flights in the air, just landed, or about to depart | read |
| `status` | Scheduled, estimated and actual times, delay, gate, terminal, belt | read |
| `delay` | Historical delay distribution for a flight | read |
| `ics` | Your flights as an iCalendar (`.ics`) file (`[FLIGHT]`, `--date`, `--upcoming`, `--past`, `--year`, `--include-archived`, `--include-following`) | read |
| `friends` | Flights of your Flighty Friends (`[NAME]`, `--upcoming`, `--limit`) | read |
| `stats` | Totals, distance, time in the air, top airlines, airports, routes | read |
| `connections` | Layovers between your flights, with minimum connection time | read |
| `airports` | Search airports by IATA, ICAO, city or name | read |
| `airlines` | Search airlines by IATA, ICAO or name | read |
| `about` | Version, mode, whether the database and a token were found (never the token) | read |
| `add` | Add a flight you're on (`<FLIGHT_NO> <DATE>`, `--force`, `--dry-run`) | write |
| `follow` | Track someone else's flight (same arguments as `add`) | write |
| `remove` | Remove a flight by id. **Experimental**, see [Safety](#safety) | destructive |
| `mcp` | Run the MCP server over stdio | — |

### Output

A readable table by default; `-o json` on any command gives the same shape the MCP tools return.

Times are shown in each airport's local zone. In JSON every timestamp is an object:

```json
{ "utc": "2031-03-18T07:05:00Z", "local": "2031-03-18T14:05:00+07:00", "tz": "Asia/Ho_Chi_Minh" }
```

Search is accent- and case-insensitive (`con dao` finds Côn Đảo).

### Calendar export

`flightydeck ics` prints an iCalendar file built from the local database, with no network call.
Each flight is one event from departure to arrival (best-known times, in UTC), with the
terminal, gate, seat, booking reference and aircraft in the notes. The event UID is the Flighty
flight id, so calendar apps that match on UID update an event when you import a newer export.
The file holds your booking references, so treat it like the tickets themselves.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | OK |
| 1 | Error (for example "Flighty is syncing, retry") |
| 2 | Bad input, caught before any network call |
| 3 | Not ready: Flighty not installed or not signed in, token expired, database schema changed, owner unresolved |
| 4 | Flighty API error (the message includes the HTTP status) |
| 5 | Refused by policy (read-only mode, remove not enabled, missing `--yes`) |
| 6 | Not found |

### Environment

| Variable | Effect |
|---|---|
| `FLIGHTY_READ_ONLY=1` | No writes at all. Overrides everything else, including `FLIGHTY_ALLOW_REMOVE`. |
| `FLIGHTY_ALLOW_REMOVE=1` | Enables the experimental `remove` command and MCP tool. |
| `FLIGHTY_OWNER_USER_ID` | Your Flighty user id, if it can't be worked out from the database (for example, a shared install). |
| `FLIGHTY_DB` | Path to the Flighty database, if it isn't in the default location. |

## MCP server

```sh
claude mcp add flightydeck -- flightydeck mcp
```

Any MCP client that can launch a stdio server works the same way: the command is `flightydeck mcp`.

| Tool | Class |
|---|---|
| `flightydeck_list_flights`, `flightydeck_get_flight`, `flightydeck_search_flights`, `flightydeck_current_flights` | read |
| `flightydeck_get_flight_status`, `flightydeck_get_delay_forecast` | read |
| `flightydeck_list_friend_flights`, `flightydeck_get_flight_stats`, `flightydeck_get_connections` | read |
| `flightydeck_search_airports`, `flightydeck_search_airlines`, `flightydeck_about` | read |
| `flightydeck_export_ics` | read |
| `flightydeck_add_flight`, `flightydeck_follow_flight` | write (not registered when read-only) |
| `flightydeck_remove_flight` | destructive (registered only with `FLIGHTY_ALLOW_REMOVE=1`) |

Every tool calls the same library function as the matching CLI command.

If your agent can run shell commands, the CLI plus the bundled skill
([`skills/flightydeck/SKILL.md`](skills/flightydeck/SKILL.md)) costs no context until it's used, unlike
an MCP server whose tool schemas load into every session.

## Safety

Every command is in one safety class, and the policy is enforced in the shared library, so the CLI
and the MCP server follow the same rules.

- **Read** is always on. The database is opened read-only, once per operation.
- **Write** (`add`, `follow`) is on unless `FLIGHTY_READ_ONLY=1`. Before subscribing it checks
  the flight isn't already tracked (`--force` overrides), and write calls are spaced out with
  backoff on rate limits. `--dry-run` only looks the flight up and reports the match; nothing
  is added, so it also works in read-only mode.
- **Destructive** (`remove`) is off unless `FLIGHTY_ALLOW_REMOVE=1`, and needs `--yes`. It is
  experimental: the sync behaviour it relies on isn't fully verified. Try it only on a flight
  you added for the purpose.
- **Token.** Your auth token is read from the local database on each call, sent only to
  `https://api.flightyapp.com` (no other host, redirects disabled), never logged or included in
  errors or JSON, and zeroized after use. An expired token exits with code 3: open Flighty to
  refresh it.

## How it works

- **Reads** come from the Flighty app's SQLite database in its macOS container. flightydeck
  checks the columns it needs before each query, so when an app update changes the schema you
  get exit code 3 naming the missing column instead of wrong data. Optional fields (weather,
  belt, tail) go blank instead of failing.
- **Writes** call Flighty's API the same way the app does: search for the flight, then subscribe
  to it as a passenger (`add`) or a follower (`follow`). The flight then syncs to your devices.

## Limitations

- macOS only, and only with the Flighty Mac app installed and signed in.
- The database layout and the API are private. A Flighty update can break flightydeck until it's
  fixed here.
- New flights appear in `list` only after the app has synced them to the local database.
- `remove` is experimental.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
scripts/check-public.sh
```

Tests run against a synthetic database built from `tests/fixtures/` (invented people and
flights). No real database is ever committed.

`scripts/check-public.sh` scans the repo for JWT-shaped strings, bearer tokens, database files,
real email addresses and non-synthetic ids in tests. Run it before pushing. To also check for
your own private terms (names, booking references), put them one per line in a file outside the
repo and run `CHECK_PUBLIC_DENYLIST=/path/to/terms.txt scripts/check-public.sh`. CI runs it on
every push.

The website is plain HTML in [`docs/`](docs/), served by GitHub Pages from the `main` branch,
`/docs` folder.

## Credits

API details learned from [CPLX/flighty-mcp-server](https://github.com/CPLX/flighty-mcp-server)
and [LukasHaas/flighty-mcp](https://github.com/LukasHaas/flighty-mcp). No code was copied.

## License

[MIT](LICENSE)
