---
name: flightydeck
description: Read and manage the user's flights from the Flighty macOS app with the `flightydeck` CLI. Use when the user asks about their flights, trips, upcoming or past flights, flight status, delays, gates, layovers, flight stats, friends' flights, airports or airlines, or wants to add or follow a flight in Flighty, or export flights to a calendar (.ics) file.
---

# flightydeck

`flightydeck` is an unofficial CLI for the Flighty Mac app. Reads come from the app's local
database; `add` and `follow` call Flighty's API. macOS only.

## Rules

- **Always pass `-o json`** and parse the result. The table output is for humans and may change.
- **Branch on the exit code**, not on the message text (see below).
- **Confirm with the user before `add`, `follow` or `remove`.** Show the flight number, date and
  what will happen, and run it only after they agree.
- **Never try to read the auth token**, from the database, the keychain or anywhere else, and
  never query the Flighty database directly. `flightydeck about` says whether a token exists; that's
  all you need.
- **Times are local to each airport.** Every timestamp in JSON is `{utc, local, tz}`. Quote
  `local` to the user and name the zone when it differs from theirs; use `utc` for arithmetic.
- Batch several reads into one shell call when you need them together.

## Intent → command

| User wants | Command |
|---|---|
| Next flights | `flightydeck list --upcoming -o json` |
| Flights in a year | `flightydeck list --year 2031 -o json` |
| One flight, seat, booking | `flightydeck get BA286 --date 2031-03-14 -o json` |
| Flights to/from somewhere | `flightydeck search --to london -o json` (accent-insensitive) |
| What's in the air now | `flightydeck current -o json` |
| Is it delayed, which gate | `flightydeck status UA901 -o json` |
| Follow a flight over time | `flightydeck watch BA286 --once -o json` for one reading. Without `--once` it keeps running until the flight lands: only start that if the user asked for a running watch |
| How often is it late | `flightydeck delay UA901 -o json` |
| Friends' flights | `flightydeck friends --upcoming -o json` |
| Travel stats | `flightydeck stats --year 2031 -o json` |
| Layovers | `flightydeck connections -o json` |
| Airport or airline lookup | `flightydeck airports "sao paulo" -o json`, `flightydeck airlines united -o json` |
| More than 50 flights | `flightydeck list --past --limit 50 --offset 50 -o json` (default limit is 50) |
| Flights as a calendar file | `flightydeck ics --upcoming > flights.ics` (no `-o json`; the output is the file) |
| Check a flight exists before adding | `flightydeck add UA901 2031-03-18 --dry-run -o json` (looks it up, adds nothing) |
| Add a flight they're on | `flightydeck add UA901 2031-03-18 -o json` after the user confirms |
| Track someone else's flight | `flightydeck follow BA286 2031-03-14 -o json` after the user confirms |
| Health check | `flightydeck about -o json` |

Run `flightydeck <command> --help` for all flags.

## Exit codes

| Code | Meaning | What to do |
|---|---|---|
| 0 | OK | Use the output |
| 1 | Error, e.g. Flighty is syncing | Retry once after a few seconds, then report |
| 2 | Bad input (no network call was made) | Fix the flight number or date format (`YYYY-MM-DD`) |
| 3 | Not ready: app not installed/signed in, token expired, schema changed, owner unresolved | Tell the user to open Flighty (refreshes the token); if the owner is unresolved, they set `FLIGHTY_OWNER_USER_ID`. Don't retry in a loop |
| 4 | Flighty API error | Report the message; don't hammer the API with retries |
| 5 | Refused by policy (read-only, remove not enabled) | Tell the user; don't change their environment to get around it |
| 6 | Not found | Say so; offer a `search` |

## Gotchas

- A flight added with `add` shows up in `list` only after the app syncs it to the local database.
  If it's missing right after adding, wait and check again, and don't add it a second time.
  (`add` refuses an already-tracked flight unless `--force`.)
- `--dry-run` on `add` and `follow` still calls Flighty's search API once per candidate airline.
  Use it to confirm a match, not to browse schedules.
- `watch` gives an estimated position while a flight is in the air (`position.estimated` is
  always true). Say "estimated" when you quote it; it is not live tracking and has no altitude.
- `ics` output contains booking references. Write it where the user asked, nowhere else.
- `remove` is experimental, off unless `FLIGHTY_ALLOW_REMOVE=1`, and needs `--yes`. Don't enable it
  on the user's behalf.
- `FLIGHTY_READ_ONLY=1` blocks every write. Respect it.
