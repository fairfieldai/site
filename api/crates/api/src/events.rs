//! Meetups and RSVPs.
//!
//! Events are copied from Discord by the reminder job (see
//! [`shared::events`]); the site lists them and the calendar feed publishes
//! them without signing in. Members RSVP to upcoming events with their site
//! account.

use std::cmp::Reverse;
use std::sync::Arc;

use axum::extract::{FromRef, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use lambda_http::tracing;
use serde::Serialize;
use serde_json::json;
use shared::events::{Attendee, Event, EventStore, RsvpError, Status};
use shared::time::now;

use crate::SiteUrl;
use crate::auth::{User, Verifier};
use crate::ics;

pub type Events = Arc<dyn EventStore>;

pub fn routes<S>() -> Router<S>
where
    Events: FromRef<S>,
    SiteUrl: FromRef<S>,
    Arc<Verifier>: FromRef<S>,
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/events", get(list))
        .route("/api/events.ics", get(calendar))
        .route("/api/events/{id}/rsvp", put(rsvp).delete(cancel_rsvp))
        .route("/api/account/rsvps", get(my_rsvps))
}

#[derive(Serialize)]
struct Listed {
    #[serde(flatten)]
    event: Event,
    upcoming: bool,
}

#[derive(Serialize)]
struct Rsvp {
    going: bool,
    rsvps: u32,
}

fn error(status: StatusCode, message: &'static str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn store_failed(error: &dyn std::fmt::Display) -> Response {
    tracing::error!(%error, "event store failed");
    self::error(StatusCode::INTERNAL_SERVER_ERROR, "couldn't load events")
}

fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "no such event")
}

/// Discord event IDs are snowflakes: decimal digits.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
}

/// Upcoming events soonest first, each marked `true`, then past events most
/// recent first. Canceled events stay listed until they would have ended, so
/// members who RSVP'd see the cancellation.
#[must_use]
pub fn arrange(events: Vec<Event>, now: i64) -> Vec<(Event, bool)> {
    let mut upcoming = Vec::new();
    let mut past = Vec::new();
    for event in events {
        match event.status {
            Status::Scheduled | Status::Active | Status::Canceled if event.ends_at > now => {
                upcoming.push(event);
            }
            Status::Canceled => {}
            Status::Scheduled | Status::Active | Status::Ended => past.push(event),
        }
    }
    upcoming.sort_by_key(|event| (event.starts_at, event.id.clone()));
    past.sort_by_key(|event| (Reverse(event.starts_at), event.id.clone()));
    let mut arranged = Vec::new();
    for event in upcoming {
        arranged.push((event, true));
    }
    for event in past {
        arranged.push((event, false));
    }
    arranged
}

async fn list(State(events): State<Events>) -> Response {
    match events.list().await {
        Ok(all) => {
            let events: Vec<Listed> = arrange(all, now())
                .into_iter()
                .map(|(event, upcoming)| Listed { event, upcoming })
                .collect();
            Json(json!({ "events": events })).into_response()
        }
        Err(error) => store_failed(&error),
    }
}

async fn calendar(State(events): State<Events>, State(site_url): State<SiteUrl>) -> Response {
    match events.list().await {
        Ok(mut all) => {
            all.sort_by_key(|event| (event.starts_at, event.id.clone()));
            (
                [
                    (header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
                    (header::CACHE_CONTROL, "public, max-age=900"),
                ],
                ics::calendar(&all, &site_url, now()),
            )
                .into_response()
        }
        Err(error) => store_failed(&error),
    }
}

fn rsvp_result(result: Result<u32, RsvpError>, going: bool) -> Response {
    match result {
        Ok(rsvps) => Json(Rsvp { going, rsvps }).into_response(),
        Err(RsvpError::NotFound) => not_found(),
        Err(RsvpError::Closed) => error(StatusCode::CONFLICT, "this event is over or canceled"),
        Err(RsvpError::Store(error)) => store_failed(&error),
    }
}

async fn rsvp(State(events): State<Events>, user: User, Path(id): Path<String>) -> Response {
    if !valid_id(&id) {
        return not_found();
    }
    let attendee = Attendee {
        user_id: user.sub,
        username: user.username,
    };
    rsvp_result(events.rsvp(&id, &attendee, now()).await, true)
}

async fn cancel_rsvp(State(events): State<Events>, user: User, Path(id): Path<String>) -> Response {
    if !valid_id(&id) {
        return not_found();
    }
    rsvp_result(events.cancel_rsvp(&id, &user.sub).await, false)
}

async fn my_rsvps(State(events): State<Events>, user: User) -> Response {
    match events.rsvps_by_user(&user.sub).await {
        Ok(ids) => Json(json!({ "events": ids })).into_response(),
        Err(error) => store_failed(&error),
    }
}
