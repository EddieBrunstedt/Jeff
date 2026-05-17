use poise::serenity_prelude as serenity;

use crate::opencritic::Game;

use super::embed::build_announcement_embed;
use super::refresh::build_refresh_action_row;

pub async fn send_announcement(
    http: &serenity::Http,
    channel_id: serenity::ChannelId,
    game: &Game,
) -> std::result::Result<serenity::Message, serenity::Error> {
    let message = serenity::CreateMessage::new()
        .embed(build_announcement_embed(game))
        .components(vec![build_refresh_action_row(game.id)]);
    channel_id.send_message(http, message).await
}
