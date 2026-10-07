//! Discord interactions and webhook events for the fairfieldct.ai bot.
//!
//! Discord signs every request with the application's Ed25519 key. Both
//! endpoints verify the signature over the timestamp and raw body before
//! parsing anything, and Discord tests this by sending bad signatures when the
//! endpoint URLs are saved.

pub mod events;
pub mod reminders;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

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

use shared::links::LinkStore;

use crate::events::{EventSource, next_meetup};

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
    #[serde(default)]
    data: Option<WebhookEventData>,
}

#[derive(Deserialize)]
struct WebhookEventData {
    user: Option<DiscordUser>,
}

#[derive(Deserialize)]
struct DiscordUser {
    id: String,
}

const APPLICATION_DEAUTHORIZED: &str = "APPLICATION_DEAUTHORIZED";

/// What the handlers share across requests.
#[derive(Clone)]
pub struct AppState {
    pub verifier: Arc<Verifier>,
    pub events: Arc<dyn EventSource>,
    pub guild_id: String,
    /// Every environment's Discord account links. Discord only sends webhook
    /// events here (prod), so a deauthorized account's link is removed from each.
    pub links: Vec<Arc<dyn LinkStore>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/discord/interactions", post(interactions))
        .route("/api/discord/events", post(webhook_events))
        .fallback(not_found)
        .with_state(state)
}

async fn interactions(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if !state.verifier.verify(&headers, &body) {
        return error(StatusCode::UNAUTHORIZED, "invalid request signature");
    }
    let Ok(interaction) = serde_json::from_slice::<Interaction>(&body) else {
        return error(StatusCode::BAD_REQUEST, "invalid interaction");
    };
    match interaction.kind {
        PING => Json(json!({ "type": PONG })).into_response(),
        APPLICATION_COMMAND => {
            let name = interaction.data.map(|data| data.name);
            Json(command_response(&state, name.as_deref()).await).into_response()
        }
        _ => error(StatusCode::BAD_REQUEST, "unsupported interaction type"),
    }
}

async fn command_response(state: &AppState, name: Option<&str>) -> Value {
    match name {
        Some("ping") => private("Pong!"),
        Some("meetup") => meetup(state).await,
        _ => private("Sorry, I don't know that command yet."),
    }
}

/// The next meetup, visible to the whole channel.
async fn meetup(state: &AppState) -> Value {
    let events = match state.events.scheduled_events().await {
        Ok(events) => events,
        Err(error) => {
            tracing::error!(%error, "couldn't load scheduled events");
            return private(
                "Sorry, I couldn't load meetups right now. Please try again in a minute.",
            );
        }
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        });
    match next_meetup(&events, now) {
        Some(event) => {
            let heading = if event.is_active() {
                format!("Happening now: {}", event.name)
            } else {
                format!("Next meetup: {}", event.name)
            };
            json!({
                "type": CHANNEL_MESSAGE_WITH_SOURCE,
                "data": {
                    "content": event.message(&heading, &state.guild_id),
                    "allowed_mentions": { "parse": [] },
                },
            })
        }
        None => private("No meetups are scheduled yet. Watch #announcements for the next one."),
    }
}

/// A reply only the person who ran the command can see.
fn private(content: &str) -> Value {
    json!({
        "type": CHANNEL_MESSAGE_WITH_SOURCE,
        "data": { "content": content, "flags": EPHEMERAL },
    })
}

async fn webhook_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !state.verifier.verify(&headers, &body) {
        return error(StatusCode::UNAUTHORIZED, "invalid request signature");
    }
    let Ok(envelope) = serde_json::from_slice::<WebhookEventEnvelope>(&body) else {
        return error(StatusCode::BAD_REQUEST, "invalid webhook event");
    };
    // Discord's endpoint check is a type 0 ping, which needs only a 204.
    let Some(event) = envelope.event.filter(|_| envelope.kind == WEBHOOK_EVENT) else {
        return StatusCode::NO_CONTENT.into_response();
    };
    tracing::info!(event = event.kind, "received Discord webhook event");
    if event.kind != APPLICATION_DEAUTHORIZED {
        return StatusCode::NO_CONTENT.into_response();
    }
    let Some(user) = event.data.and_then(|data| data.user) else {
        return error(StatusCode::BAD_REQUEST, "deauthorization without a user");
    };
    // Discord retries failed deliveries, so any storage error returns 500
    // after trying every store; removing an already-removed link is a no-op.
    let mut failed = false;
    for links in &state.links {
        match links.remove_by_discord(&user.id).await {
            Ok(removed) => {
                tracing::info!(
                    discord_user = user.id,
                    removed = removed.is_some(),
                    "removed Discord link"
                );
            }
            Err(error) => {
                tracing::error!(%error, discord_user = user.id, "couldn't remove Discord link");
                failed = true;
            }
        }
    }
    if failed {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "couldn't remove link");
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
