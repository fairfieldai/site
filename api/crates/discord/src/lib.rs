//! Discord interactions and webhook events for the fairfieldct.ai bot.
//!
//! Discord signs every request with the application's Ed25519 key. Both
//! endpoints verify the signature over the timestamp and raw body before
//! parsing anything, and Discord tests this by sending bad signatures when the
//! endpoint URLs are saved.

use std::sync::Arc;

use aws_lc_rs::signature::{ED25519, UnparsedPublicKey};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use lambda_http::tracing;
use serde::Deserialize;
use serde_json::{Value, json};

const PING: u8 = 1;
const APPLICATION_COMMAND: u8 = 2;
const PONG: u8 = 1;
const CHANNEL_MESSAGE_WITH_SOURCE: u8 = 4;
const EPHEMERAL: u32 = 1 << 6;
const WEBHOOK_EVENT: u8 = 1;

/// Checks Discord's request signatures against the application's public key.
pub struct Verifier {
    key: [u8; 32],
}

impl Verifier {
    /// Parses the hex public key from the Discord developer portal.
    #[must_use]
    pub fn from_hex(public_key: &str) -> Option<Self> {
        let key = decode_hex(public_key)?.try_into().ok()?;
        Some(Self { key })
    }

    fn verify(&self, headers: &HeaderMap, body: &[u8]) -> bool {
        let header = |name| headers.get(name).and_then(|value| value.to_str().ok());
        let (Some(signature), Some(timestamp)) = (
            header("x-signature-ed25519"),
            header("x-signature-timestamp"),
        ) else {
            return false;
        };
        let Some(signature) = decode_hex(signature) else {
            return false;
        };
        let message = [timestamp.as_bytes(), body].concat();
        UnparsedPublicKey::new(&ED25519, &self.key)
            .verify(&message, &signature)
            .is_ok()
    }
}

// An odd length leaves a final half pair, which `get` rejects.
fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect()
}

#[derive(Deserialize)]
struct Interaction {
    #[serde(rename = "type")]
    kind: u8,
    data: Option<CommandData>,
}

#[derive(Deserialize)]
struct CommandData {
    name: String,
}

#[derive(Deserialize)]
struct WebhookEventEnvelope {
    #[serde(rename = "type")]
    kind: u8,
    event: Option<WebhookEventBody>,
}

#[derive(Deserialize)]
struct WebhookEventBody {
    #[serde(rename = "type")]
    kind: String,
}

pub fn router(verifier: Verifier) -> Router {
    Router::new()
        .route("/api/discord/interactions", post(interactions))
        .route("/api/discord/events", post(events))
        .fallback(not_found)
        .with_state(Arc::new(verifier))
}

async fn interactions(
    State(verifier): State<Arc<Verifier>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !verifier.verify(&headers, &body) {
        return error(StatusCode::UNAUTHORIZED, "invalid request signature");
    }
    let Ok(interaction) = serde_json::from_slice::<Interaction>(&body) else {
        return error(StatusCode::BAD_REQUEST, "invalid interaction");
    };
    match interaction.kind {
        PING => Json(json!({ "type": PONG })).into_response(),
        APPLICATION_COMMAND => {
            let name = interaction.data.map(|data| data.name);
            Json(command_response(name.as_deref())).into_response()
        }
        _ => error(StatusCode::BAD_REQUEST, "unsupported interaction type"),
    }
}

fn command_response(name: Option<&str>) -> Value {
    let content = match name {
        Some("ping") => "Pong!",
        _ => "Sorry, I don't know that command yet.",
    };
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": { "content": content, "flags": EPHEMERAL },
    })
}

async fn events(
    State(verifier): State<Arc<Verifier>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !verifier.verify(&headers, &body) {
        return error(StatusCode::UNAUTHORIZED, "invalid request signature");
    }
    let Ok(envelope) = serde_json::from_slice::<WebhookEventEnvelope>(&body) else {
        return error(StatusCode::BAD_REQUEST, "invalid webhook event");
    };
    // Discord's endpoint check is a type 0 ping; both need only a 204.
    if envelope.kind == WEBHOOK_EVENT
        && let Some(event) = envelope.event
    {
        tracing::info!(event = event.kind, "received Discord webhook event");
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not found")
}

fn error(status: StatusCode, message: &'static str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

#[cfg(test)]
mod tests;
