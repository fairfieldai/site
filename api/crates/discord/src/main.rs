use std::env;

use discord::Verifier;
use lambda_http::{Error, run, tracing};

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing::init_default_subscriber();

    let public_key = env::var("DISCORD_PUBLIC_KEY")?;
    let verifier =
        Verifier::from_hex(&public_key).ok_or("DISCORD_PUBLIC_KEY is not a 32-byte hex key")?;

    run(discord::router(verifier)).await
}
