use std::env;
use std::sync::Arc;
use std::time::Duration;

use api::AppState;
use api::auth::{CognitoKeys, Verifier};
use lambda_http::{Error, run, tracing};

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing::init_default_subscriber();

    let issuer = env::var("COGNITO_ISSUER")?;
    let client_id = env::var("COGNITO_CLIENT_ID")?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let keys = CognitoKeys::new(http, &issuer);
    let state = AppState {
        verifier: Arc::new(Verifier::new(issuer, client_id, Box::new(keys))),
    };

    run(api::router(state)).await
}
