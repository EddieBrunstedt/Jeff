use anyhow::{Context, Result};
use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};

use crate::opencritic::OpenCriticGame;

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
pub struct OpenCriticSubscription {
    pub channel_id: String,
    pub threshold: f64,
    pub min_reviews: i64,
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
        CREATE TABLE IF NOT EXISTS opencritic_subscription (
          id INTEGER PRIMARY KEY CHECK (id = 1),
          channel_id TEXT NOT NULL,
          threshold REAL NOT NULL,
          min_reviews INTEGER NOT NULL
        )
        "#,
    )
    .execute(&pool)
    .await
    .context("Failed to create opencritic_subscription table")?;

    Ok(pool)
}

pub async fn ping(pool: &SqlitePool) -> Result<()> {
    sqlx::query("SELECT 1")
        .execute(pool)
        .await
        .context("Database ping failed")?;
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
    .with_context(|| format!("Failed to insert OpenCritic decision for game {}", game.id))?;

    Ok(())
}

pub async fn get_opencritic_subscription(
    pool: &SqlitePool,
) -> Result<Option<OpenCriticSubscription>> {
    let row: Option<(String, f64, i64)> = sqlx::query_as(
        "SELECT channel_id, threshold, min_reviews FROM opencritic_subscription WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .context("Failed to load OpenCritic subscription")?;

    Ok(row.map(
        |(channel_id, threshold, min_reviews)| OpenCriticSubscription {
            channel_id,
            threshold,
            min_reviews,
        },
    ))
}

pub async fn insert_opencritic_subscription(
    pool: &SqlitePool,
    channel_id: &str,
    threshold: f64,
    min_reviews: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO opencritic_subscription (id, channel_id, threshold, min_reviews) VALUES (1, ?, ?, ?)",
    )
    .bind(channel_id)
    .bind(threshold)
    .bind(min_reviews)
    .execute(pool)
    .await
    .context("Failed to store OpenCritic subscription")?;

    Ok(())
}

pub async fn update_opencritic_subscription_settings(
    pool: &SqlitePool,
    threshold: f64,
    min_reviews: i64,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE opencritic_subscription SET threshold = ?, min_reviews = ? WHERE id = 1",
    )
    .bind(threshold)
    .bind(min_reviews)
    .execute(pool)
    .await
    .context("Failed to update OpenCritic subscription settings")?;

    Ok(result.rows_affected() > 0)
}

pub async fn delete_opencritic_subscription(pool: &SqlitePool) -> Result<bool> {
    let result = sqlx::query("DELETE FROM opencritic_subscription WHERE id = 1")
        .execute(pool)
        .await
        .context("Failed to delete OpenCritic subscription")?;

    Ok(result.rows_affected() > 0)
}
