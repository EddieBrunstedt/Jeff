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
PRETTY_LOG=false
```

## Build and run

```bash
cargo run --release
```

Slash commands are registered to the guild specified by `GUILD_ID` on startup. The bot supports a single subscribed channel at a time.

## Commands

```text
/jeff subscribe score:<n> [min_reviews:<n>]
/jeff threshold score:<n> [min_reviews:<n>]
/jeff unsubscribe
/jeff status
/jeff health
```

`/jeff subscribe` enables alerts in the current channel for games whose top critic score is at or above `score` and which have at least `min_reviews` reviews (`min_reviews` defaults to 10). The bot scans OpenCritic for PC games released in the current and previous calendar year. Games that already meet the threshold at subscribe time are recorded as known so they do not post — only future threshold crossings produce alerts.

`/jeff threshold` updates the threshold of the active subscription. Lowering it records any games that newly qualify under the lower threshold as known (no announcement) so the channel does not get flooded with a backlog. Raising it does not unrecord anything — games already announced stay announced.

`/jeff unsubscribe` disables alerts.

`/jeff status` shows the current subscription state.

`/jeff health` reports bot health (uptime, database, gateway latency, OpenCritic API quota, last poll).

The configuration commands (`subscribe`, `threshold`, `unsubscribe`) require the **Manage Channels** permission.
