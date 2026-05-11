# Jeff

A Discord bot that watches OpenCritic and posts in a channel when a PC game's top critic score crosses a configured threshold. Written in Rust.

## Requirements

- Rust toolchain (stable)
- A Discord bot application from <https://discord.com/developers/applications>. When inviting the bot to your server, grant it the `applications.commands` and `bot` scopes, plus permission to send messages and embed links in the target channel.
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

`/jeff subscribe` enables alerts in the current channel at the given top critic score (`min_reviews` defaults to 10). On subscribe, currently-qualifying games are snapshotted as baseline so only new threshold crossings post.

`/jeff threshold` changes the threshold for the active subscription. Lowering it re-baselines the newly-eligible band so existing high-scoring games do not flood the channel.

`/jeff unsubscribe` disables alerts.

`/jeff status` shows the current subscription state.

`/jeff health` reports bot health (uptime, database, gateway latency, OpenCritic API quota, last poll).

The configuration commands (`subscribe`, `threshold`, `unsubscribe`) require the **Manage Channels** permission.
