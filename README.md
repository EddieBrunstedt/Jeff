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
OPENCRITIC_THRESHOLD=85
OPENCRITIC_MIN_REVIEWS=10
OPENCRITIC_SCAN_PREVIOUS_YEAR=true
OPENCRITIC_POLL_INTERVAL_SECONDS=21600
PRETTY_LOG=false
```

The bot starts background OpenCritic polling automatically. The first poll creates a baseline and posts nothing. Alerts are sent only to channels that have subscribed with `/opencritic_subscribe`.

## Commands

```text
/healthcheck
/opencritic_status
/opencritic_subscribe
/opencritic_unsubscribe
/opencritic_test_post
/opencritic_check post:false
/opencritic_check post:true
```

`/opencritic_subscribe` enables OpenCritic alerts in the current channel. `/opencritic_unsubscribe` disables them.

`/opencritic_check post:false` is a dry run. `post:true` posts new qualifying games to subscribed channels and records them as handled.

`/opencritic_test_post` posts a sample alert in the current channel from a random baseline-suppressed game without recording a decision or announcement.
