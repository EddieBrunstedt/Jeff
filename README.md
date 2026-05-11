# Jeff

A Discord bot for sharing video game info.

Early goal: watch OpenCritic and post when a game reaches a good review-score threshold.

## Configuration

Required:

```text
DISCORD_TOKEN=
GUILD_ID=
RAPIDAPI_KEY=
OPENCRITIC_POLL_INTERVAL_SECONDS=
```

Optional:

```text
DATABASE_URL=sqlite://jeff.sqlite?mode=rwc
PRETTY_LOG=false
```

The bot is designed for a single Discord server with a single subscribed channel. Commands are registered to the guild specified by `GUILD_ID`.

The bot starts background OpenCritic polling automatically. The poll is a no-op until the bot is subscribed in a channel via `/jeff subscribe`.

## Commands

```text
/jeff subscribe score:<n> [min_reviews:<n>]
/jeff threshold score:<n> [min_reviews:<n>]
/jeff unsubscribe
/jeff status
/jeff health
```

`/jeff subscribe` enables OpenCritic alerts in the current channel at the given top critic score threshold. The bot supports one subscription at a time. On subscribe, currently-qualifying games are snapshotted as baseline so only new threshold crossings post.

`/jeff threshold` changes the threshold for the active subscription. Lowering the threshold re-baselines the newly-eligible band so existing high-scoring games do not flood the channel.

`/jeff unsubscribe` disables alerts.

`/jeff status` shows whether the bot is subscribed and the current threshold.

`/jeff health` reports bot health (uptime, database, gateway latency, OpenCritic API quota, last poll).

The configuration commands (`subscribe`, `threshold`, `unsubscribe`) require the **Manage Channels** permission.

## Migration

If you have an existing `jeff.sqlite` from a previous version, delete it before starting this build. The schema has been simplified and the old data is not upgraded automatically. Re-running `/jeff subscribe` will rebuild the baseline.
