# Jeff

A Discord bot for sharing video game info.

Early goal: watch OpenCritic and post when a game reaches a good review-score threshold.

## Configuration

Required:

```text
DISCORD_TOKEN=
GUILD_ID=
RAPIDAPI_KEY=
```

Optional:

```text
DATABASE_URL=sqlite://jeff.sqlite?mode=rwc
OPENCRITIC_POLL_INTERVAL_SECONDS=21600
PRETTY_LOG=false
```

The bot starts background OpenCritic polling automatically. Thresholds are configured per channel through `/opencritic_subscribe`; the poll is a no-op until at least one channel is subscribed.

## Commands

```text
/healthcheck
/opencritic_status
/opencritic_subscribe score:<n> [min_reviews:<n>]
/opencritic_set_threshold score:<n> [min_reviews:<n>]
/opencritic_unsubscribe
```

`/opencritic_subscribe score:<n>` enables OpenCritic alerts in the current channel at the given top critic score threshold. On subscribe, currently-qualifying games are snapshotted as baseline-suppressed so only new threshold crossings post.

`/opencritic_set_threshold score:<n>` changes the threshold for the current channel. Lowering the threshold re-baselines the newly-eligible band so existing high-scoring games do not flood the channel.

`/opencritic_unsubscribe` disables alerts in the current channel.

The configuration commands (`subscribe`, `set_threshold`, `unsubscribe`) require the **Manage Channels** permission.
