# Jeff

A Discord bot that watches OpenCritic and posts in a channel when a PC game's top critic score crosses a configured threshold. Written in Rust.

## Requirements

- Rust toolchain (stable)
- A Discord bot application from <https://discord.com/developers/applications>. When generating the invite URL (OAuth2 → URL Generator in the developer portal), tick both the `bot` and `applications.commands` scopes — `bot` lets the application join the server, `applications.commands` is what makes the `/jeff` commands appear. Also grant the bot permission to send messages and embed links in the target channel.
- A RapidAPI account with access to the OpenCritic API (<https://rapidapi.com/digiwalls/api/opencritic-api>). The free tier allows 200 requests/day.

## Configuration

Required environment variables:

```text
DISCORD_TOKEN=
GUILD_ID=
RAPIDAPI_KEY=
OPENCRITIC_POLL_INTERVAL_SECONDS=
```

Each poll typically costs ~6 RapidAPI requests. On the free 200-requests/day tier, intervals of `7200` (2h) or longer leave comfortable headroom.

Optional:

```text
DATABASE_URL=sqlite://jeff.sqlite?mode=rwc
LOG_FORMAT=json
RUST_LOG=info,jeff=debug,serenity=warn,sqlx=warn
JEFF_ENABLE_DEBUG_POST=false
JEFF_DISABLE_POLLING=false
```

`LOG_FORMAT` accepts `json` (default — one structured object per line, suitable
for piping into tools like Nushell or `jq`) or `pretty` (human-readable, for
local `cargo run`).

`RUST_LOG` controls verbosity using the standard `tracing-subscriber` env
filter. The default if unset is `info,jeff=debug,serenity=warn,sqlx=warn`.
Set `jeff=trace` to also dump raw HTTP response bodies.

`JEFF_ENABLE_DEBUG_POST=true` registers the `/jeff debug_post` subcommand. It posts three bundled fixture games to the current channel so the alert layout can be iterated on without consuming RapidAPI quota.

`JEFF_DISABLE_POLLING=true` skips spawning the background OpenCritic poller. Useful during local development so restarts don't burn RapidAPI quota on automatic scans. Slash commands and the Refresh button still work.

## Build and run

```bash
cargo run --release
```

Slash commands are registered to the guild specified by `GUILD_ID` on startup. The bot supports a single subscribed channel at a time.

## Commands

```text
/jeff subscribe score:<n> min_reviews:<n> min_recommended:<n>
/jeff threshold [score:<n>] [min_reviews:<n>] [min_recommended:<n>]
/jeff unsubscribe
/jeff status
/jeff health
```

`/jeff subscribe` enables alerts in the current channel for games whose top critic score is at or above `score`, which have at least `min_reviews` reviews, and whose critics-recommended percentage is at or above `min_recommended` (0–100). All three parameters are mandatory — the admin must set each axis explicitly. The bot scans OpenCritic for PC games released in the current and previous calendar year. Games that already meet the gate at subscribe time are recorded as known so they do not post — only future threshold crossings produce alerts.

`/jeff threshold` updates one or more values of the active subscription. Each parameter is optional; omitted values keep their current setting. Lowering any threshold records any games that newly qualify as known (no announcement) so the channel does not get flooded with a backlog. Raising values does not unrecord anything — games already announced stay announced.

`/jeff unsubscribe` disables alerts.

`/jeff status` shows the current subscription state.

`/jeff health` reports bot health (uptime, database, gateway latency, OpenCritic API quota, last poll).

The configuration commands (`subscribe`, `threshold`, `unsubscribe`) require the **Manage Channels** permission.
