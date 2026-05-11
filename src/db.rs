use anyhow::{Context, Result};
use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};

use crate::opencritic::OpenCriticGame;

const BASELINE_COMPLETED_KEY: &str = "opencritic_baseline_completed_at";
const OPENCRITIC_FEATURE: &str = "opencritic";

#[derive(Debug, Clone, Copy)]
pub enum DecisionStatus {
    BaselineSuppressed,
    Announced,
}

impl DecisionStatus {
    fn as_str(self) -> &'static str {
        match self {
            DecisionStatus::BaselineSuppressed => "baseline_suppressed",
            DecisionStatus::Announced => "announced",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FeatureSubscription {
    pub guild_id: String,
    pub channel_id: String,
}

#[derive(Debug, Clone)]
pub struct SuppressedDecision {
    pub name: String,
    pub url: Option<String>,
    pub score: f64,
    pub review_count: i64,
}

pub async fn connect_and_migrate(database_url: &str) -> Result<SqlitePool> {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await
        .with_context(|| format!("Failed to connect to database at {database_url}"))?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS opencritic_decisions (
          game_id INTEGER PRIMARY KEY,
          status TEXT NOT NULL CHECK (status IN ('baseline_suppressed', 'announced')),
          name TEXT NOT NULL,
          url TEXT,
          score REAL NOT NULL,
          review_count INTEGER NOT NULL,
          percent_recommended REAL,
          decided_at TEXT NOT NULL
        )
        "#,
    )
    .execute(&pool)
    .await
    .context("Failed to create opencritic_decisions table")?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS bot_meta (
          key TEXT PRIMARY KEY,
          value TEXT NOT NULL
        )
        "#,
    )
    .execute(&pool)
    .await
    .context("Failed to create bot_meta table")?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS feature_subscriptions (
          feature TEXT NOT NULL,
          guild_id TEXT NOT NULL,
          channel_id TEXT NOT NULL,
          created_by TEXT NOT NULL,
          created_at TEXT NOT NULL,
          PRIMARY KEY (feature, channel_id)
        )
        "#,
    )
    .execute(&pool)
    .await
    .context("Failed to create feature_subscriptions table")?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS opencritic_announcements (
          game_id INTEGER NOT NULL,
          guild_id TEXT NOT NULL,
          channel_id TEXT NOT NULL,
          discord_message_id TEXT NOT NULL,
          announced_at TEXT NOT NULL,
          PRIMARY KEY (game_id, channel_id)
        )
        "#,
    )
    .execute(&pool)
    .await
    .context("Failed to create opencritic_announcements table")?;

    Ok(pool)
}

pub async fn is_opencritic_baseline_complete(pool: &SqlitePool) -> Result<bool> {
    let value: Option<(String,)> = sqlx::query_as("SELECT value FROM bot_meta WHERE key = ?")
        .bind(BASELINE_COMPLETED_KEY)
        .fetch_optional(pool)
        .await
        .context("Failed to read OpenCritic baseline metadata")?;

    Ok(value.is_some())
}

pub async fn mark_opencritic_baseline_complete(
    pool: &SqlitePool,
    completed_at: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO bot_meta (key, value)
        VALUES (?, ?)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
        "#,
    )
    .bind(BASELINE_COMPLETED_KEY)
    .bind(completed_at)
    .execute(pool)
    .await
    .context("Failed to store OpenCritic baseline metadata")?;

    Ok(())
}

pub async fn opencritic_decision_exists(pool: &SqlitePool, game_id: i64) -> Result<bool> {
    let value: Option<(i64,)> =
        sqlx::query_as("SELECT game_id FROM opencritic_decisions WHERE game_id = ? LIMIT 1")
            .bind(game_id)
            .fetch_optional(pool)
            .await
            .context("Failed to check OpenCritic decision")?;

    Ok(value.is_some())
}

pub async fn insert_opencritic_decision(
    pool: &SqlitePool,
    game: &OpenCriticGame,
    status: DecisionStatus,
    decided_at: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO opencritic_decisions (
          game_id,
          status,
          name,
          url,
          score,
          review_count,
          percent_recommended,
          decided_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(game_id) DO NOTHING
        "#,
    )
    .bind(game.id)
    .bind(status.as_str())
    .bind(&game.name)
    .bind(&game.url)
    .bind(game.top_critic_score)
    .bind(game.num_reviews)
    .bind(game.percent_recommended)
    .bind(decided_at)
    .execute(pool)
    .await
    .with_context(|| format!("Failed to insert OpenCritic decision for {}", game.id))?;

    Ok(())
}

pub async fn upsert_opencritic_subscription(
    pool: &SqlitePool,
    guild_id: &str,
    channel_id: &str,
    created_by: &str,
    created_at: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO feature_subscriptions (feature, guild_id, channel_id, created_by, created_at)
        VALUES (?, ?, ?, ?, ?)
        ON CONFLICT(feature, channel_id) DO UPDATE SET
          guild_id = excluded.guild_id,
          created_by = excluded.created_by,
          created_at = excluded.created_at
        "#,
    )
    .bind(OPENCRITIC_FEATURE)
    .bind(guild_id)
    .bind(channel_id)
    .bind(created_by)
    .bind(created_at)
    .execute(pool)
    .await
    .context("Failed to store OpenCritic subscription")?;

    Ok(())
}

pub async fn delete_opencritic_subscription(pool: &SqlitePool, channel_id: &str) -> Result<bool> {
    let result =
        sqlx::query("DELETE FROM feature_subscriptions WHERE feature = ? AND channel_id = ?")
            .bind(OPENCRITIC_FEATURE)
            .bind(channel_id)
            .execute(pool)
            .await
            .context("Failed to delete OpenCritic subscription")?;

    Ok(result.rows_affected() > 0)
}

pub async fn list_opencritic_subscriptions(pool: &SqlitePool) -> Result<Vec<FeatureSubscription>> {
    let subscriptions = sqlx::query_as::<_, (String, String)>(
        "SELECT guild_id, channel_id FROM feature_subscriptions WHERE feature = ? ORDER BY channel_id",
    )
    .bind(OPENCRITIC_FEATURE)
    .fetch_all(pool)
    .await
    .context("Failed to list OpenCritic subscriptions")?
    .into_iter()
    .map(|(guild_id, channel_id)| FeatureSubscription {
        guild_id,
        channel_id,
    })
    .collect();

    Ok(subscriptions)
}

pub async fn opencritic_announcement_exists(
    pool: &SqlitePool,
    game_id: i64,
    channel_id: &str,
) -> Result<bool> {
    let value: Option<(i64,)> = sqlx::query_as(
        "SELECT game_id FROM opencritic_announcements WHERE game_id = ? AND channel_id = ? LIMIT 1",
    )
    .bind(game_id)
    .bind(channel_id)
    .fetch_optional(pool)
    .await
    .context("Failed to check OpenCritic announcement")?;

    Ok(value.is_some())
}

pub async fn insert_opencritic_announcement(
    pool: &SqlitePool,
    game_id: i64,
    guild_id: &str,
    channel_id: &str,
    discord_message_id: &str,
    announced_at: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO opencritic_announcements (
          game_id,
          guild_id,
          channel_id,
          discord_message_id,
          announced_at
        )
        VALUES (?, ?, ?, ?, ?)
        ON CONFLICT(game_id, channel_id) DO NOTHING
        "#,
    )
    .bind(game_id)
    .bind(guild_id)
    .bind(channel_id)
    .bind(discord_message_id)
    .bind(announced_at)
    .execute(pool)
    .await
    .context("Failed to store OpenCritic announcement")?;

    Ok(())
}

pub async fn list_baseline_suppressed_opencritic(
    pool: &SqlitePool,
) -> Result<Vec<SuppressedDecision>> {
    let rows: Vec<(String, Option<String>, f64, i64)> = sqlx::query_as(
        r#"
        SELECT name, url, score, review_count
        FROM opencritic_decisions
        WHERE status = 'baseline_suppressed'
        ORDER BY score DESC
        "#,
    )
    .fetch_all(pool)
    .await
    .context("Failed to list baseline-suppressed OpenCritic games")?;

    Ok(rows
        .into_iter()
        .map(|(name, url, score, review_count)| SuppressedDecision {
            name,
            url,
            score,
            review_count,
        })
        .collect())
}

pub async fn count_opencritic_decisions(pool: &SqlitePool) -> Result<(i64, i64, i64, i64)> {
    let baseline: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM opencritic_decisions WHERE status = 'baseline_suppressed'",
    )
    .fetch_one(pool)
    .await
    .context("Failed to count baseline OpenCritic decisions")?;

    let announced: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM opencritic_decisions WHERE status = 'announced'")
            .fetch_one(pool)
            .await
            .context("Failed to count announced OpenCritic decisions")?;

    let subscriptions: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM feature_subscriptions WHERE feature = ?")
            .bind(OPENCRITIC_FEATURE)
            .fetch_one(pool)
            .await
            .context("Failed to count OpenCritic subscriptions")?;

    let announcement_deliveries: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM opencritic_announcements")
            .fetch_one(pool)
            .await
            .context("Failed to count OpenCritic announcement deliveries")?;

    Ok((
        baseline.0,
        announced.0,
        subscriptions.0,
        announcement_deliveries.0,
    ))
}
