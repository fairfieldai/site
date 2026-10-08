//! Members' meetup announcement emails: opting in and out from the account
//! page, and one-click unsubscribe links in each email.
//!
//! The reminder job sends the emails (see [`shared::subscribers`]).

use std::fmt::Write;
use std::sync::Arc;

use axum::extract::{FromRef, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use lambda_http::tracing;
use serde::Deserialize;
use serde_json::json;
use shared::subscribers::{Subscriber, Subscribers};

use crate::auth::{User, Verifier};

pub type Subscriptions = Arc<dyn Subscribers>;

/// Hex characters in an unsubscribe token (32 random bytes).
const TOKEN_LENGTH: usize = 64;

pub fn routes<S>() -> Router<S>
where
    Subscriptions: FromRef<S>,
    Arc<Verifier>: FromRef<S>,
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/account/email", get(status).put(update))
        .route("/api/email/unsubscribe/{token}", post(unsubscribe))
}

#[derive(Deserialize)]
struct Preferences {
    announcements: bool,
}

fn store_failed(error: &dyn std::fmt::Display) -> Response {
    tracing::error!(%error, "subscriber store failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "couldn't update your email preferences" })),
    )
        .into_response()
}

fn preferences(announcements: bool) -> Response {
    Json(json!({ "announcements": announcements })).into_response()
}

/// A new unsubscribe token.
///
/// # Errors
///
/// If the system random number generator fails.
pub fn new_token() -> Result<String, aws_lc_rs::error::Unspecified> {
    let mut bytes = [0u8; TOKEN_LENGTH / 2];
    aws_lc_rs::rand::fill(&mut bytes)?;
    let mut token = String::with_capacity(TOKEN_LENGTH);
    for byte in bytes {
        // Writing to a String can't fail.
        let _ = write!(token, "{byte:02x}");
    }
    Ok(token)
}

async fn status(State(subscribers): State<Subscriptions>, user: User) -> Response {
    match subscribers.get(&user.sub).await {
        Ok(subscriber) => preferences(subscriber.is_some()),
        Err(error) => store_failed(&error),
    }
}

async fn update(
    State(subscribers): State<Subscriptions>,
    user: User,
    Json(request): Json<Preferences>,
) -> Response {
    let result = if request.announcements {
        let Ok(token) = new_token() else {
            return store_failed(&"random number generator failed");
        };
        subscribers
            .subscribe(Subscriber {
                user_id: user.sub,
                username: user.username,
                token,
            })
            .await
    } else {
        subscribers.unsubscribe(&user.sub).await.map(|_| ())
    };
    match result {
        Ok(()) => preferences(request.announcements),
        Err(error) => store_failed(&error),
    }
}

/// The unsubscribe link's target. Mail apps that support one-click
/// unsubscribe (RFC 8058) POST here directly; the site's `/unsubscribe/` page
/// does the same after the member confirms, so link scanners that follow GET
/// links can't unsubscribe anyone.
async fn unsubscribe(
    State(subscribers): State<Subscriptions>,
    Path(token): Path<String>,
) -> Response {
    if token.len() != TOKEN_LENGTH || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Json(json!({ "unsubscribed": false })).into_response();
    }
    match subscribers.unsubscribe_token(&token).await {
        Ok(removed) => Json(json!({ "unsubscribed": removed })).into_response(),
        Err(error) => store_failed(&error),
    }
}
