use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

use anyhow::{Context, Result, bail};
use reqwest::header::{HeaderMap, HeaderValue};
use serde::Deserialize;
use tracing::{debug, error, trace, warn};

use crate::util::{non_empty_str, truncate_bytes};

const LOG_BODY_TRUNCATE: usize = 2048;
const PAGE_SIZE: usize = 20;
const RATE_LIMIT_WARN_THRESHOLD: i64 = 20;

#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    rapidapi_key: String,
    last_rate_limit_remaining: Arc<Mutex<Option<i64>>>,
}

#[derive(Debug, Clone)]
pub struct Scan {
    pub games: Vec<Game>,
    pub pages_requested: usize,
    pub rate_limit_remaining: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ImageSet {
    #[serde(default)]
    pub og: Option<String>,
    #[serde(default)]
    pub xl: Option<String>,
    #[serde(default)]
    pub lg: Option<String>,
    #[serde(default)]
    pub md: Option<String>,
    #[serde(default)]
    pub sm: Option<String>,
    #[serde(default)]
    pub xs: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Images {
    #[serde(default)]
    pub masthead: ImageSet,
    #[serde(default)]
    pub banner: ImageSet,
    #[serde(default)]
    pub square: ImageSet,
    #[serde(default, rename = "box")]
    pub box_art: ImageSet,
    #[serde(default)]
    pub logo: ImageSet,
    #[serde(default)]
    pub screenshots: Vec<ImageSet>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Screenshot {
    #[serde(default, rename = "fullRes")]
    pub full_res: Option<String>,
    #[serde(default)]
    pub thumbnail: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Platform {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, rename = "shortName")]
    pub short_name: Option<String>,
    #[serde(default, rename = "releaseDate")]
    pub release_date: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Genre {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Company {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Game {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub images: Images,
    #[serde(default)]
    pub screenshots: Vec<Screenshot>,
    #[serde(default, rename = "mastheadScreenshot")]
    pub masthead_screenshot: Option<Screenshot>,
    #[serde(default, rename = "bannerScreenshot")]
    pub banner_screenshot: Option<Screenshot>,
    #[serde(default, rename = "squareScreenshot")]
    pub square_screenshot: Option<Screenshot>,
    #[serde(default, rename = "logoScreenshot")]
    pub logo_screenshot: Option<Screenshot>,
    #[serde(default, rename = "firstReleaseDate", alias = "releaseDate")]
    pub first_release_date: Option<String>,
    #[serde(default, rename = "Platforms", alias = "platforms")]
    pub platforms: Vec<Platform>,
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
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "Genres", alias = "genres")]
    pub genres: Vec<Genre>,
    #[serde(default, rename = "Companies", alias = "companies")]
    pub companies: Vec<Company>,
}

impl Game {
    pub fn qualifies(
        &self,
        threshold: f64,
        min_reviews: i64,
        min_percent_recommended: f64,
    ) -> bool {
        self.top_critic_score >= threshold
            && self.num_reviews >= min_reviews
            && self.percent_recommended >= min_percent_recommended
    }

    pub fn pc_release_date(&self) -> Option<&str> {
        pc_platform_release_date(&self.platforms)
    }

    pub fn primary_image_url(&self) -> Option<String> {
        [
            self.images.masthead.xl.as_deref(),
            self.images.masthead.lg.as_deref(),
            self.images.masthead.og.as_deref(),
            self.images.masthead.md.as_deref(),
            self.images.masthead.sm.as_deref(),
            self.images.masthead.xs.as_deref(),
            self.images.banner.og.as_deref(),
            self.images.banner.sm.as_deref(),
            self.masthead_screenshot
                .as_ref()
                .and_then(|image| image.full_res.as_deref()),
            self.masthead_screenshot
                .as_ref()
                .and_then(|image| image.thumbnail.as_deref()),
            self.banner_screenshot
                .as_ref()
                .and_then(|image| image.full_res.as_deref()),
            self.banner_screenshot
                .as_ref()
                .and_then(|image| image.thumbnail.as_deref()),
            self.images.square.lg.as_deref(),
            self.images.square.og.as_deref(),
            self.images.square.sm.as_deref(),
            self.images.square.xs.as_deref(),
            self.square_screenshot
                .as_ref()
                .and_then(|image| image.full_res.as_deref()),
            self.square_screenshot
                .as_ref()
                .and_then(|image| image.thumbnail.as_deref()),
            self.images.box_art.og.as_deref(),
            self.images.box_art.sm.as_deref(),
            self.images.logo.og.as_deref(),
            self.logo_screenshot
                .as_ref()
                .and_then(|image| image.full_res.as_deref()),
            self.logo_screenshot
                .as_ref()
                .and_then(|image| image.thumbnail.as_deref()),
            self.images
                .screenshots
                .first()
                .and_then(|image| image.og.as_deref()),
            self.images
                .screenshots
                .first()
                .and_then(|image| image.sm.as_deref()),
            self.screenshots
                .first()
                .and_then(|image| image.full_res.as_deref()),
            self.screenshots
                .first()
                .and_then(|image| image.thumbnail.as_deref()),
        ]
        .into_iter()
        .flatten()
        .find_map(normalize_opencritic_image_url)
    }

    pub fn fill_missing_from(&mut self, fallback: &Self) {
        if self.name.trim().is_empty() {
            self.name = fallback.name.clone();
        }
        if self.platforms.is_empty() {
            self.platforms = fallback.platforms.clone();
        }
        if optional_string_is_empty(&self.first_release_date) {
            self.first_release_date = self
                .pc_release_date()
                .map(ToOwned::to_owned)
                .or_else(|| fallback.pc_release_date().map(ToOwned::to_owned))
                .or_else(|| fallback.first_release_date.clone())
                .or_else(|| first_platform_release_date(&self.platforms))
                .or_else(|| first_platform_release_date(&fallback.platforms));
        }
        if self.percent_recommended < 0.0 && fallback.percent_recommended >= 0.0 {
            self.percent_recommended = fallback.percent_recommended;
        }
        if self.num_reviews <= 0 && fallback.num_reviews > 0 {
            self.num_reviews = fallback.num_reviews;
        }
        if self.top_critic_score < 0.0 && fallback.top_critic_score >= 0.0 {
            self.top_critic_score = fallback.top_critic_score;
        }
        if self.tier.trim().is_empty() {
            self.tier = fallback.tier.clone();
        }
        if optional_string_is_empty(&self.url) {
            self.url = fallback.url.clone();
        }
        if optional_string_is_empty(&self.description) {
            self.description = fallback.description.clone();
        }
        if self.genres.is_empty() {
            self.genres = fallback.genres.clone();
        }
        if self.companies.is_empty() {
            self.companies = fallback.companies.clone();
        }
    }

    pub fn box_art_url(&self) -> Option<String> {
        [
            self.images.box_art.og.as_deref(),
            self.images.box_art.lg.as_deref(),
            self.images.box_art.md.as_deref(),
            self.images.box_art.sm.as_deref(),
            self.images.box_art.xs.as_deref(),
        ]
        .into_iter()
        .flatten()
        .find_map(normalize_opencritic_image_url)
    }

    pub fn genre_names(&self) -> Vec<&str> {
        self.genres
            .iter()
            .filter_map(|genre| genre.name.as_deref())
            .filter_map(non_empty_str)
            .collect()
    }

    pub fn developers(&self) -> Vec<&str> {
        self.companies_of_kind("DEVELOPER")
    }

    pub fn publishers(&self) -> Vec<&str> {
        self.companies_of_kind("PUBLISHER")
    }

    fn companies_of_kind(&self, kind: &str) -> Vec<&str> {
        self.companies
            .iter()
            .filter(|company| {
                company
                    .kind
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case(kind))
            })
            .filter_map(|company| company.name.as_deref())
            .filter_map(non_empty_str)
            .collect()
    }
}

fn optional_string_is_empty(value: &Option<String>) -> bool {
    value
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
}

fn first_platform_release_date(platforms: &[Platform]) -> Option<String> {
    platforms
        .iter()
        .filter_map(|platform| platform.release_date.as_deref())
        .filter_map(non_empty_str)
        .map(ToOwned::to_owned)
        .next()
}

fn pc_platform_release_date(platforms: &[Platform]) -> Option<&str> {
    platforms
        .iter()
        .find(|platform| {
            platform
                .short_name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case("pc"))
                || platform
                    .name
                    .as_deref()
                    .is_some_and(|name| name.eq_ignore_ascii_case("pc"))
        })
        .and_then(|platform| platform.release_date.as_deref())
        .and_then(non_empty_str)
}

fn normalize_opencritic_image_url(value: &str) -> Option<String> {
    let value = non_empty_str(value)?;

    if value.starts_with("https://") || value.starts_with("http://") {
        Some(value.to_string())
    } else if value.starts_with("//") {
        Some(format!("https:{value}"))
    } else if value.starts_with('/') {
        Some(format!("https://img.opencritic.com{value}"))
    } else {
        Some(format!("https://img.opencritic.com/{value}"))
    }
}

impl Client {
    pub fn new(rapidapi_key: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            rapidapi_key,
            last_rate_limit_remaining: Arc::new(Mutex::new(None)),
        }
    }

    pub fn last_rate_limit_remaining(&self) -> Option<i64> {
        *self.last_rate_limit_remaining.lock().unwrap()
    }

    #[tracing::instrument(skip(self), fields(game_id))]
    pub async fn fetch_game(&self, game_id: i64) -> Result<Game> {
        let url = format!("https://opencritic-api.p.rapidapi.com/game/{game_id}");
        let started = Instant::now();
        let response = self
            .http
            .get(&url)
            .headers(self.headers()?)
            .send()
            .await
            .context("Failed to send OpenCritic game details request")?;

        let status = response.status();
        let rate_limit_remaining = self.record_rate_limit_remaining(&response);

        let body = response
            .text()
            .await
            .context("Failed to read OpenCritic game details response body")?;

        debug!(
            status = status.as_u16(),
            body_bytes = body.len(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            rate_limit_remaining,
            "opencritic game fetched",
        );
        trace!(body = %body, "opencritic game raw response");

        if !status.is_success() {
            error!(
                status = status.as_u16(),
                body = %truncate_bytes(&body, LOG_BODY_TRUNCATE),
                "opencritic game request failed",
            );
            bail!("OpenCritic game details request failed with {status}");
        }

        serde_json::from_str::<Game>(&body).map_err(|err| {
            error!(
                error = ?err,
                body = %truncate_bytes(&body, LOG_BODY_TRUNCATE),
                "opencritic game parse failed",
            );
            anyhow::anyhow!("Failed to parse OpenCritic game details response: {err}")
        })
    }

    #[tracing::instrument(
        skip(self),
        fields(
            years = ?years,
            threshold,
            min_reviews,
            min_percent_recommended,
        ),
    )]
    pub async fn scan_years(
        &self,
        years: &[i32],
        threshold: f64,
        min_reviews: i64,
        min_percent_recommended: f64,
    ) -> Result<Scan> {
        let mut games_by_id = HashMap::new();
        let mut pages_requested = 0;
        let mut rate_limit_remaining = None;

        for year in years {
            let year_scan = self
                .scan_year(*year, threshold, min_reviews, min_percent_recommended)
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

        Ok(Scan {
            games,
            pages_requested,
            rate_limit_remaining,
        })
    }

    #[tracing::instrument(
        skip(self),
        fields(year, threshold, min_reviews, min_percent_recommended)
    )]
    async fn scan_year(
        &self,
        year: i32,
        threshold: f64,
        min_reviews: i64,
        min_percent_recommended: f64,
    ) -> Result<Scan> {
        let mut games = Vec::new();
        let mut pages_requested = 0;
        let mut rate_limit_remaining;
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
                    .filter(|game| game.qualifies(threshold, min_reviews, min_percent_recommended)),
            );

            if page_len < PAGE_SIZE || last_score < threshold {
                break;
            }

            skip += PAGE_SIZE;
        }

        Ok(Scan {
            games,
            pages_requested,
            rate_limit_remaining,
        })
    }

    #[tracing::instrument(skip(self), fields(year, skip))]
    async fn fetch_game_page(&self, year: i32, skip: usize) -> Result<Page> {
        let started = Instant::now();
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
        let rate_limit_remaining = self.record_rate_limit_remaining(&response);

        let body = response
            .text()
            .await
            .context("Failed to read OpenCritic page response body")?;

        debug!(
            status = status.as_u16(),
            body_bytes = body.len(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            rate_limit_remaining,
            "opencritic page fetched",
        );
        trace!(body = %body, "opencritic page raw response");

        if !status.is_success() {
            error!(
                status = status.as_u16(),
                body = %truncate_bytes(&body, LOG_BODY_TRUNCATE),
                "opencritic page request failed",
            );
            bail!("OpenCritic request failed with {status}");
        }

        let games = serde_json::from_str::<Vec<Game>>(&body).map_err(|err| {
            error!(
                error = ?err,
                body = %truncate_bytes(&body, LOG_BODY_TRUNCATE),
                "opencritic page parse failed",
            );
            anyhow::anyhow!("Failed to parse OpenCritic response: {err}")
        })?;

        debug!(items = games.len(), "opencritic page parsed");

        Ok(Page {
            games,
            rate_limit_remaining,
        })
    }

    fn record_rate_limit_remaining(&self, response: &reqwest::Response) -> Option<i64> {
        let rate_limit_remaining = response
            .headers()
            .get("x-ratelimit-requests-remaining")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<i64>().ok())?;

        let prior = {
            let mut guard = self.last_rate_limit_remaining.lock().unwrap();
            let prior = *guard;
            *guard = Some(rate_limit_remaining);
            prior
        };

        if rate_limit_remaining < RATE_LIMIT_WARN_THRESHOLD {
            let prior_above_threshold = prior
                .map(|value| value >= RATE_LIMIT_WARN_THRESHOLD)
                .unwrap_or(true);
            if prior_above_threshold {
                warn!(
                    rate_limit_remaining,
                    threshold = RATE_LIMIT_WARN_THRESHOLD,
                    "OpenCritic rate limit remaining dropped below threshold",
                );
            }
        }

        Some(rate_limit_remaining)
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

struct Page {
    games: Vec<Game>,
    rate_limit_remaining: Option<i64>,
}
