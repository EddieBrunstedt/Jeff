use chrono::Utc;
use poise::serenity_prelude as serenity;

use crate::opencritic::Game;

const DESCRIPTION_MAX_LEN: usize = 400;

pub fn build_announcement_embed(game: &Game) -> serenity::CreateEmbed {
    let url = game
        .url
        .as_deref()
        .and_then(non_empty_str)
        .unwrap_or("https://opencritic.com")
        .to_string();

    let mut embed = serenity::CreateEmbed::new()
        .title(&game.name)
        .url(url)
        .author(serenity::CreateEmbedAuthor::new("OpenCritic").url("https://opencritic.com"));

    if let Some(color) = embed_color_for_tier(&game.tier) {
        embed = embed.colour(color);
    }

    if let Some(description) = game.description.as_deref().and_then(non_empty_str) {
        embed = embed.description(truncate_description(description));
    }

    if let Some(image_url) = game.primary_image_url() {
        embed = embed.image(image_url);
    }

    if let Some(box_art) = game.box_art_url() {
        embed = embed.thumbnail(box_art);
    }

    let tier_value = if game.tier.is_empty() {
        "N/A".to_string()
    } else {
        game.tier.clone()
    };

    embed = embed
        .field(
            "Critic Average",
            format!("{:.1}", game.top_critic_score),
            true,
        )
        .field(
            "Recommended",
            if game.percent_recommended >= 0.0 {
                format!("{:.0}%", game.percent_recommended)
            } else {
                "N/A".to_string()
            },
            true,
        )
        .field("Reviews", game.num_reviews.to_string(), true)
        .field("Tier", tier_value, true);

    let release_date = game
        .pc_release_date()
        .or_else(|| game.first_release_date.as_deref().and_then(non_empty_str))
        .map(format_opencritic_date)
        .unwrap_or_else(|| "unknown".to_string());
    embed = embed.field("PC Release", release_date, true);

    let genres = game.genre_names();
    if !genres.is_empty() {
        embed = embed.field("Genres", genres.join(", "), true);
    }

    let developers = game.developers();
    if !developers.is_empty() {
        embed = embed.field("Developer", developers.join(", "), true);
    }

    let publishers = game.publishers();
    if !publishers.is_empty() {
        embed = embed.field("Publisher", publishers.join(", "), true);
    }

    embed = embed.field(
        "Steam",
        format!("[Search]({})", steam_search_url(&game.name)),
        true,
    );

    embed
        .footer(serenity::CreateEmbedFooter::new("Last refreshed"))
        .timestamp(Utc::now())
}

fn embed_color_for_tier(tier: &str) -> Option<serenity::Colour> {
    match tier {
        "Mighty" => Some(serenity::Colour::new(0xE6B800)),
        "Strong" => Some(serenity::Colour::new(0x2ECC71)),
        "Fair" => Some(serenity::Colour::new(0x3498DB)),
        "Weak" => Some(serenity::Colour::new(0x95A5A6)),
        _ => None,
    }
}

fn truncate_description(value: &str) -> String {
    if value.chars().count() <= DESCRIPTION_MAX_LEN {
        return value.to_string();
    }

    let truncated: String = value.chars().take(DESCRIPTION_MAX_LEN).collect();
    let cut_at = truncated
        .rfind(char::is_whitespace)
        .unwrap_or(truncated.len());
    let mut result = truncated[..cut_at].trim_end().to_string();
    result.push('…');
    result
}

fn steam_search_url(game_name: &str) -> String {
    let mut encoded = String::new();
    for byte in game_name.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(*byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }

    format!("https://store.steampowered.com/search/?term={encoded}")
}

fn format_opencritic_date(value: &str) -> String {
    value
        .split('T')
        .next()
        .filter(|date| !date.is_empty())
        .unwrap_or(value)
        .to_string()
}

fn non_empty_str(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}
