# OpenCritic Discord Bot Notes

## Current goal

Post a Discord alert when a PC game newly qualifies for a configured OpenCritic score threshold after the bot has established its initial baseline.

Default trigger:

```text
topCriticScore >= 85
numReviews >= 10
```

Use `topCriticScore` as the primary signal. Display `percentRecommended` as context only.

## Current implementation status

Implemented in Rust with:

- Discord commands through `poise`
- OpenCritic API calls through `reqwest`
- local SQLite state through `sqlx`
- background polling through Tokio

Implemented commands:

```text
/healthcheck
/opencritic_status
/opencritic_subscribe
/opencritic_unsubscribe
/opencritic_check post:false
/opencritic_check post:true
```

Important behavior:

- The background poller starts on bot startup.
- The first OpenCritic poll creates a baseline and posts nothing.
- Alerts are delivered only to channels subscribed with `/opencritic_subscribe`.
- `/opencritic_unsubscribe` disables alerts in the current channel.
- `/opencritic_check post:false` dry-runs the poll.
- `/opencritic_check post:true` runs the live delivery path manually.

## Required configuration

```text
DISCORD_TOKEN=
GUILD_ID=
RAPIDAPI_KEY=
```

Optional configuration:

```text
DATABASE_URL=sqlite://jeff.sqlite?mode=rwc
OPENCRITIC_THRESHOLD=85
OPENCRITIC_MIN_REVIEWS=10
OPENCRITIC_SCAN_PREVIOUS_YEAR=true
OPENCRITIC_POLL_INTERVAL_SECONDS=21600
PRETTY_LOG=false
```

There is intentionally no `DISCORD_ALERT_CHANNEL_ID`. Channel routing is stored by subscription commands.

## OpenCritic API

Use RapidAPI:

```text
https://opencritic-api.p.rapidapi.com
```

Headers:

```text
X-RapidAPI-Key: $RAPIDAPI_KEY
X-RapidAPI-Host: opencritic-api.p.rapidapi.com
```

Observed request limit:

```text
x-ratelimit-requests-limit: 200
```

Treat this as roughly 200 requests/day.

## Browse endpoint

Endpoint shape:

```bash
curl -s --get 'https://opencritic-api.p.rapidapi.com/game' \
  --data-urlencode 'platforms=pc' \
  --data-urlencode 'time=2026' \
  --data-urlencode 'sort=topCriticScore' \
  --data-urlencode 'skip=0' \
  -H "X-RapidAPI-Key: $RAPIDAPI_KEY" \
  -H 'X-RapidAPI-Host: opencritic-api.p.rapidapi.com'
```

Response is a JSON array. Useful fields:

```json
{
  "id": 18716,
  "name": "Resident Evil Requiem",
  "firstReleaseDate": "2026-02-27T00:00:00.000Z",
  "percentRecommended": 95.28,
  "numReviews": 212,
  "topCriticScore": 89.18,
  "tier": "Mighty",
  "url": "https://opencritic.com/game/18716/resident-evil-requiem"
}
```

Unrated games use sentinel values:

```json
{
  "topCriticScore": -1,
  "numReviews": 0,
  "percentRecommended": -1,
  "tier": ""
}
```

## Pagination and scanning

The `/game` endpoint returns about 20 items per page and paginates with `skip`:

```text
skip=0
skip=20
skip=40
...
```

There is no total count or `next` field. Stop when:

- the response is empty
- the response has fewer than 20 items
- the page is score-sorted and the last score is below the configured threshold

Current scan strategy:

```text
current year + previous year
platforms=pc
sort=topCriticScore
```

OpenCritic appears to support:

- `time=last90`
- specific years, e.g. `time=2026`
- `time=all`

It does not appear to support arbitrary windows such as `last180` or `last365`.

Observed with threshold `85`:

```text
2026: 2 requests
2025: 4 requests
current year + previous year: 6 requests/poll
```

With a 200 request/day limit:

```text
6 requests/poll * 4 polls/day = 24 requests/day
6 requests/poll * 8 polls/day = 48 requests/day
6 requests/poll * 12 polls/day = 72 requests/day
```

The default interval is 6 hours.

## Baseline behavior

The bot should not announce already-high games that existed before it started.

First OpenCritic poll:

1. Fetch qualifying games.
2. Insert them into `opencritic_decisions` as `baseline_suppressed`.
3. Mark `opencritic_baseline_completed_at` in `bot_meta`.
4. Send no Discord messages.

After baseline:

1. Fetch qualifying games.
2. Skip any game already in `opencritic_decisions`.
3. Send new qualifying games to subscribed channels.
4. Insert per-channel delivery rows in `opencritic_announcements`.
5. Insert a global `announced` decision for the game after live delivery succeeds.

If there are no subscribed channels during live polling, new qualifying games are not marked announced.

## Why upcoming games are not scanned

Upcoming scans are not needed for threshold alerts.

The bot does not need to have seen a game before it had a score. The rule is simply:

```text
if game qualifies now
and baseline is complete
and game_id has no stored decision
then announce it to subscribed channels
```

This handles games whose first public scored state is already above the threshold after an embargo lifts.

For reference, testing `https://opencritic.com/game/19996/forza-horizon-6` showed:

- `/game/19996` returned `topCriticScore = -1`, `numReviews = 0`
- `/review/game/19996` returned `[]`

Hidden embargoed reviews were not exposed through the public API in that test.

## SQLite state model

Use SQLite as a local, self-contained database.

Implemented tables:

```sql
CREATE TABLE IF NOT EXISTS opencritic_decisions (
  game_id INTEGER PRIMARY KEY,
  status TEXT NOT NULL CHECK (status IN ('baseline_suppressed', 'announced')),
  name TEXT NOT NULL,
  url TEXT,
  score REAL NOT NULL,
  review_count INTEGER NOT NULL,
  percent_recommended REAL,
  decided_at TEXT NOT NULL,
  discord_channel_id TEXT,
  discord_message_id TEXT
);

CREATE TABLE IF NOT EXISTS bot_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS feature_subscriptions (
  feature TEXT NOT NULL,
  guild_id TEXT NOT NULL,
  channel_id TEXT NOT NULL,
  created_by TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (feature, channel_id)
);

CREATE TABLE IF NOT EXISTS opencritic_announcements (
  game_id INTEGER NOT NULL,
  guild_id TEXT NOT NULL,
  channel_id TEXT NOT NULL,
  discord_message_id TEXT NOT NULL,
  announced_at TEXT NOT NULL,
  PRIMARY KEY (game_id, channel_id)
);
```

Notes:

- `opencritic_decisions` is the global decision ledger.
- `feature_subscriptions` is intentionally generic for future features.
- `opencritic_announcements` tracks per-channel deliveries.
- The `discord_channel_id` and `discord_message_id` columns on `opencritic_decisions` are legacy from the first pass and are currently not required for multi-channel delivery.

## Current Discord message

Current message format:

```text
{name} hit {rounded_score} OpenCritic

Top Critic Average: {score}
Recommended: {percentRecommended}
Reviews: {numReviews}
Tier: {tier}
Release date: {firstReleaseDate}

{url}
```

## Future review detail enrichment

Only fetch review details for games that are about to be announced. Do not fetch review details for every scanned game.

Endpoint:

```text
/review/game/{game_id}
```

Example:

```bash
curl -s 'https://opencritic-api.p.rapidapi.com/review/game/18716' \
  -H "X-RapidAPI-Key: $RAPIDAPI_KEY" \
  -H 'X-RapidAPI-Host: opencritic-api.p.rapidapi.com'
```

Potential useful fields:

- `score`
- `ScoreFormat.scoreDisplay`
- `Outlet.name`
- `alias`
- `snippet`
- `externalUrl`

Future Discord messages can include most positive and most critical reviews.

## Suggested next steps

1. Run the bot locally and register slash commands.
2. Run `/opencritic_status`.
3. Run `/opencritic_check post:false` to verify API access and baseline state.
4. Run `/opencritic_subscribe` in the target alert channel.
5. Use `/opencritic_check post:true` only when ready to exercise live delivery.
6. Consider replacing the legacy Discord columns in `opencritic_decisions` with a migration later.
7. Add review-detail enrichment after basic delivery is verified.
