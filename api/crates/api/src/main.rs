use std::env;
use std::sync::Arc;
use std::time::Duration;

use api::AppState;
use api::auth::{CognitoKeys, Verifier};
use api::discord_link::{DiscordHttp, DiscordLinking};
use lambda_http::{Error, run, tracing};
use shared::links::DynamoLinks;
use shared::ssm::Parameter;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing::init_default_subscriber();

    let issuer = env::var("COGNITO_ISSUER")?;
    let client_id = env::var("COGNITO_CLIENT_ID")?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let keys = CognitoKeys::new(http, &issuer);

    // Linking is configured only where the environment has a Discord
    // application; the client secret is read from SSM on first use.
    let discord = match env::var("DISCORD_APPLICATION_ID") {
        Ok(application_id) => {
            let config = aws_config::load_from_env().await;
            let secret = Parameter::new(
                aws_sdk_ssm::Client::new(&config),
                env::var("DISCORD_CLIENT_SECRET_PARAMETER")?,
            );
            let oauth =
                DiscordHttp::new(application_id, secret, env::var("DISCORD_REDIRECT_URI")?)?;
            let links = DynamoLinks::new(
                aws_sdk_dynamodb::Client::new(&config),
                env::var("TABLE_NAME")?,
            );
            Some(Arc::new(DiscordLinking {
                oauth: Arc::new(oauth),
                links: Arc::new(links),
                manage_role_connection: env::var("DISCORD_MANAGE_ROLE_CONNECTION")? == "true",
            }))
        }
        Err(_) => None,
    };

    let state = AppState {
        verifier: Arc::new(Verifier::new(issuer, client_id, Box::new(keys))),
        discord,
    };

    run(api::router(state)).await
}
