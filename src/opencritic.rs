use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, bail};
use reqwest::header::{HeaderMap, HeaderValue};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct OpenCriticClient {
    http: reqwest::Client,
    rapidapi_key: String,
    last_rate_limit_remaining: Arc<Mutex<Option<String>>>,
}

#[derive(Debug, Clone)]
pub struct OpenCriticScan {
    pub games: Vec<OpenCriticGame>,
    pub pages_requested: usize,
    pub years: Vec<i32>,
    pub rate_limit_remaining: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpenCriticGame {
    pub id: i64,
    pub name: String,
    #[serde(default, rename = "firstReleaseDate")]
    pub first_release_date: Option<String>,
    #[serde(default, rename = "percentRecommended")]
    pub percent_recommended: f64,
    #[serde(default, rename = "numReviews")]
    pub num_reviews: i64,
    #[serde(default, rename = "topCriticScore")]
    pub top_critic_score: f64,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub url: Option<String>,
}

impl OpenCriticGame {
    pub fn qualifies(&self, threshold: f64, minimum_reviews: i64) -> bool {
        self.top_critic_score >= threshold && self.num_reviews >= minimum_reviews
    }
}

impl OpenCriticClient {
    pub fn new(rapidapi_key: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            rapidapi_key,
            last_rate_limit_remaining: Arc::new(Mutex::new(None)),
        }
    }

    pub fn last_rate_limit_remaining(&self) -> Option<String> {
        self.last_rate_limit_remaining.lock().unwrap().clone()
    }

    pub async fn scan_years(
        &self,
        years: &[i32],
        threshold: f64,
        minimum_reviews: i64,
    ) -> Result<OpenCriticScan> {
        let mut games_by_id = HashMap::new();
        let mut pages_requested = 0;
        let mut rate_limit_remaining = None;

        for year in years {
            let year_scan = self
                .scan_year(*year, threshold, minimum_reviews)
                .await
                .with_context(|| format!("Failed to scan OpenCritic year {year}"))?;

            pages_requested += year_scan.pages_requested;
            rate_limit_remaining = year_scan.rate_limit_remaining;

            for game in year_scan.games {
                games_by_id.entry(game.id).or_insert(game);
            }
        }

        let mut games: Vec<_> = games_by_id.into_values().collect();
        games.sort_by(|a, b| b.top_critic_score.total_cmp(&a.top_critic_score));

        Ok(OpenCriticScan {
            games,
            pages_requested,
            years: years.to_vec(),
            rate_limit_remaining,
        })
    }

    async fn scan_year(
        &self,
        year: i32,
        threshold: f64,
        minimum_reviews: i64,
    ) -> Result<OpenCriticScan> {
        let mut games = Vec::new();
        let mut pages_requested = 0;
        let mut rate_limit_remaining: Option<String>;
        let mut skip = 0;

        loop {
            let response = self.fetch_game_page(year, skip).await?;
            pages_requested += 1;
            rate_limit_remaining = response.rate_limit_remaining;

            if response.games.is_empty() {
                break;
            }

            let last_score = response
                .games
                .last()
                .map(|game| game.top_critic_score)
                .unwrap_or_default();
            let page_len = response.games.len();

            games.extend(
                response
                    .games
                    .into_iter()
                    .filter(|game| game.qualifies(threshold, minimum_reviews)),
            );

            if page_len < 20 || last_score < threshold {
                break;
            }

            skip += 20;
        }

        Ok(OpenCriticScan {
            games,
            pages_requested,
            years: vec![year],
            rate_limit_remaining,
        })
    }

    async fn fetch_game_page(&self, year: i32, skip: usize) -> Result<OpenCriticPage> {
        let response = self
            .http
            .get("https://opencritic-api.p.rapidapi.com/game")
            .query(&[
                ("platforms", "pc".to_string()),
                ("time", year.to_string()),
                ("sort", "topCriticScore".to_string()),
                ("skip", skip.to_string()),
            ])
            .headers(self.headers()?)
            .send()
            .await
            .context("Failed to send OpenCritic request")?;

        let status = response.status();
        let rate_limit_remaining = response
            .headers()
            .get("x-ratelimit-requests-remaining")
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        if let Some(value) = &rate_limit_remaining {
            *self.last_rate_limit_remaining.lock().unwrap() = Some(value.clone());
        }

        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_string());
            bail!("OpenCritic request failed with {status}: {body}");
        }

        let games = response
            .json::<Vec<OpenCriticGame>>()
            .await
            .context("Failed to parse OpenCritic response")?;

        Ok(OpenCriticPage {
            games,
            rate_limit_remaining,
        })
    }

    fn headers(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-RapidAPI-Key",
            HeaderValue::from_str(&self.rapidapi_key)
                .context("Invalid RAPIDAPI_KEY header value")?,
        );
        headers.insert(
            "X-RapidAPI-Host",
            HeaderValue::from_static("opencritic-api.p.rapidapi.com"),
        );
        Ok(headers)
    }
}

struct OpenCriticPage {
    games: Vec<OpenCriticGame>,
    rate_limit_remaining: Option<String>,
}
