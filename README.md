# Jeff

> A focused Discord bot for surfacing the PC games that are breaking through on OpenCritic.

![Rust](https://img.shields.io/badge/Rust-2024-orange)
![Discord](https://img.shields.io/badge/Discord-slash%20commands-5865F2)
![OpenCritic](https://img.shields.io/badge/OpenCritic-powered-blue)

Jeff watches OpenCritic, applies your quality gates, and posts polished Discord alerts when a PC game crosses the line. It is built for low-noise game discovery: subscribe once, snapshot the current backlog, then only hear about future threshold crossings.

## Highlights

- **Signal-first alerts**: gate on critic average, review count, and critics-recommended percentage.
- **No backlog spam**: subscribing or changing thresholds baselines already-qualifying games instead of dumping old results into chat.
- **Rich Discord embeds**: score, tier, reviews, recommendation percentage, release date, genres, developer, publisher, artwork, OpenCritic link, and Steam search.
- **One-click refresh**: each announcement includes a Refresh button that re-fetches OpenCritic details and edits the embed in place.
- **Operationally safe**: SQLite persistence, structured logs, health checks, quota visibility, channel-gone auto-unsubscribe, and retry-storm protection.
- **Guild-scoped slash commands**: commands are registered directly to your configured server for fast iteration.

## How it works

1. Jeff registers `/jeff` slash commands in the configured Discord guild.
2. An admin subscribes a channel with explicit thresholds.
3. Jeff scans PC games from the current and previous calendar year and records the current qualifying set as a baseline.
4. A background poll repeats on your configured interval.
5. When a not-yet-seen game qualifies, Jeff fetches full game details, posts an announcement, and records the decision in SQLite.

Jeff currently supports **one active subscription channel** per running bot instance.

## Requirements

- A recent stable Rust toolchain with Rust 2024 edition support.
- A Discord application and bot token from <https://discord.com/developers/applications>.
- A RapidAPI key with access to the OpenCritic API: <https://rapidapi.com/digiwalls/api/opencritic-api>.
- A Discord server ID (`GUILD_ID`) where commands should be registered.

When creating the Discord invite URL, enable both OAuth2 scopes:

- `bot`
- `applications.commands`

The bot needs these channel permissions where alerts will be posted:

- View Channel
- Send Messages
- Embed Links

## Quick start

```bash
git clone <repo-url>
cd jeff
cargo build --release
```

Create your environment configuration:

```bash
export DISCORD_TOKEN="your-discord-bot-token"
export GUILD_ID="123456789012345678"
export RAPIDAPI_KEY="your-rapidapi-key"
export OPENCRITIC_POLL_INTERVAL_SECONDS="21600"

# Optional but useful for local development
export LOG_FORMAT="pretty"
export RUST_LOG="info,jeff=debug,serenity=warn,sqlx=warn"
```

Run the bot:

```bash
cargo run --release
```

Then subscribe the current Discord channel:

```text
/jeff subscribe min_score:80 min_reviews:10 min_recommended:70
```

Slash commands are registered on startup. Because Jeff uses guild-scoped commands, they should appear quickly after the bot connects.

## Configuration

### Required

| Variable                           | Description                                                    |
| ---------------------------------- | -------------------------------------------------------------- |
| `DISCORD_TOKEN`                    | Discord bot token. Keep it secret.                             |
| `GUILD_ID`                         | Discord guild/server ID where `/jeff` commands are registered. |
| `RAPIDAPI_KEY`                     | RapidAPI key used for OpenCritic requests.                     |
| `OPENCRITIC_POLL_INTERVAL_SECONDS` | Delay between scheduled OpenCritic scans.                      |

### Optional

| Variable                        | Default                                   | Description                                                                                       |
| ------------------------------- | ----------------------------------------- | ------------------------------------------------------------------------------------------------- |
| `DATABASE_URL`                  | `sqlite://jeff.sqlite?mode=rwc`           | SQLite database URL. Tables are created automatically.                                            |
| `LOG_FORMAT`                    | `json`                                    | `json` for structured logs or `pretty` for local readable logs.                                   |
| `RUST_LOG`                      | `info,jeff=debug,serenity=warn,sqlx=warn` | Standard `tracing-subscriber` filter. Use `jeff=trace` to include raw OpenCritic response bodies. |
| `JEFF_ENABLE_DEBUG_POST`        | `false`                                   | Registers `/jeff debug_post`, which posts bundled fixture alerts without using RapidAPI.          |
| `JEFF_DISABLE_POLLING`          | `false`                                   | Prevents the background poller from starting. Slash commands and Refresh still work.              |
| `JEFF_REFRESH_COOLDOWN_SECONDS` | `60`                                      | Per-game Refresh button cooldown. Set to `0` to disable. Stored in memory and reset on restart.   |

### Poll interval and API quota

A scan requests OpenCritic list pages for the current and previous year. New announcements and manual refreshes make extra detail requests. If your RapidAPI plan is limited, choose a conservative interval and watch `/jeff health` plus the RapidAPI dashboard.

For a 200 requests/day plan, `7200` seconds (2 hours) or longer is a sensible starting point.

## Commands

| Command                                                               | Permission            | What it does                                                                                           |
| --------------------------------------------------------------------- | --------------------- | ------------------------------------------------------------------------------------------------------ |
| `/jeff subscribe min_score:<n> min_reviews:<n> min_recommended:<n>`   | Manage Channels       | Enables alerts in the current channel and baselines existing matches.                                  |
| `/jeff threshold [min_score:<n>] [min_reviews:<n>] [min_recommended:<n>]` | Manage Channels    | Updates one or more gates and baselines newly qualifying games.                                        |
| `/jeff unsubscribe`                                                   | Manage Channels       | Disables alerts and removes the active subscription.                                                   |
| `/jeff status`                                                        | None enforced by Jeff | Shows the active subscription and thresholds.                                                          |
| `/jeff health`                                                        | None enforced by Jeff | Reports uptime, database status, Discord latency, OpenCritic quota, subscription state, and last poll. |
| `/jeff debug_post`                                                    | Manage Channels       | Development-only fixture poster. Registered only when `JEFF_ENABLE_DEBUG_POST=true`.                   |

Threshold ranges:

- `min_score`: `1.0` to `100.0`
- `min_reviews`: `1` or greater
- `min_recommended`: `0.0` to `100.0`

## Alert behavior

Jeff records every qualifying game decision in `opencritic_decisions`:

- `baseline_suppressed`: the game already qualified when subscribing or rebaselining.
- `announced`: Jeff attempted to announce the game.

This is intentional. If Discord rejects an announcement for a transient channel/message error, Jeff records the game anyway to avoid repeatedly posting or retrying the same alert forever. If Discord reports that the channel no longer exists, Jeff automatically unsubscribes instead.

## Development workflow

Recommended local settings:

```bash
export LOG_FORMAT="pretty"
export JEFF_DISABLE_POLLING="true"
export JEFF_ENABLE_DEBUG_POST="true"
```

Useful commands:

```bash
cargo fmt
cargo clippy --all-targets --all-features
cargo test
cargo run
```

With `JEFF_ENABLE_DEBUG_POST=true`, use this command in Discord to preview announcement layout without spending OpenCritic quota:

```text
/jeff debug_post
```

## Project map

| Path                     | Responsibility                                                         |
| ------------------------ | ---------------------------------------------------------------------- |
| `src/main.rs`            | Startup, Discord client, command registration, event routing.          |
| `src/config.rs`          | Environment parsing and defaults.                                      |
| `src/db.rs`              | SQLite connection, schema creation, subscription and decision storage. |
| `src/opencritic.rs`      | RapidAPI/OpenCritic client, scan logic, game normalization.            |
| `src/poller.rs`          | Scheduled scans, baselining, announcement decisions.                   |
| `src/commands/jeff.rs`   | `/jeff` slash commands.                                                |
| `src/discord/embed.rs`   | Announcement embed construction.                                       |
| `src/discord/refresh.rs` | Refresh button handling and cooldowns.                                 |
| `fixtures/opencritic/`   | Local OpenCritic game payloads for debug posts.                        |
