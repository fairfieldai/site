use std::env;
use std::sync::Arc;

use discord::events::DiscordEvents;
use discord::ssm::Parameter;
use discord::{AppState, Verifier};
use lambda_http::{Error, run, tracing};

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing::init_default_subscriber();

    let public_key = env::var("DISCORD_PUBLIC_KEY")?;
    let verifier =
        Verifier::from_hex(&public_key).ok_or("DISCORD_PUBLIC_KEY is not a 32-byte hex key")?;
    let guild_id = env::var("DISCORD_GUILD_ID")?;

    // The bot token is fetched on the first /meetup, so signature checks and
    // PINGs never wait on SSM.
    let config = aws_config::load_from_env().await;
    let token = Parameter::new(
        aws_sdk_ssm::Client::new(&config),
        env::var("DISCORD_BOT_TOKEN_PARAMETER")?,
    );
    let state = AppState {
        verifier: Arc::new(verifier),
        events: Arc::new(DiscordEvents::new(guild_id.clone(), token)?),
        guild_id,
    };

    run(discord::router(state)).await
}
