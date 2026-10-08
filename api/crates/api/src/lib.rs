//! HTTP API served by API Gateway and CloudFront under `/api`.
//!
//! CloudFront forwards the full request path, so every route is mounted under
//! `/api`.

pub mod auth;
pub mod discord_link;
pub mod email;
pub mod events;
mod ics;

use std::sync::Arc;

use axum::extract::FromRef;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::auth::{User, Verifier};
use crate::discord_link::Linking;
use crate::email::Subscriptions;
use crate::events::Events;

/// The site's origin, e.g. `https://www.fairfieldct.ai`, for absolute links.
pub type SiteUrl = Arc<str>;

#[derive(Clone)]
pub struct AppState {
    pub verifier: Arc<Verifier>,
    /// Discord account linking, when this environment has it.
    pub discord: Linking,
    pub events: Events,
    pub subscribers: Subscriptions,
    pub site_url: SiteUrl,
}

impl FromRef<AppState> for Arc<Verifier> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.verifier)
    }
}

impl FromRef<AppState> for Linking {
    fn from_ref(state: &AppState) -> Self {
        state.discord.clone()
    }
}

impl FromRef<AppState> for Events {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.events)
    }
}

impl FromRef<AppState> for Subscriptions {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.subscribers)
    }
}

impl FromRef<AppState> for SiteUrl {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.site_url)
    }
}

#[derive(Debug, Serialize)]
struct Status {
    status: &'static str,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: &'static str,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/me", get(me))
        .merge(discord_link::routes())
        .merge(events::routes())
        .merge(email::routes())
        .fallback(not_found)
        .with_state(state)
}

async fn health() -> Json<Status> {
    Json(Status { status: "ok" })
}

async fn me(user: User) -> Json<User> {
    Json(user)
}

async fn not_found() -> (StatusCode, Json<ErrorBody>) {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorBody { error: "not found" }),
    )
}

#[cfg(test)]
mod tests;
