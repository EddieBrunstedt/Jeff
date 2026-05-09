use crate::context::Context;
use anyhow::Result;

#[poise::command(slash_command)]
pub async fn healthcheck(ctx: Context<'_>) -> Result<()> {
    let author_id = ctx.author().id;
    let response = format!("Hello Mr. {author_id}, I am waving!");
    ctx.say(response).await?;
    Ok(())
}
