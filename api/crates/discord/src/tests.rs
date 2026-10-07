use std::fmt::Write as _;

use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{AppState, Verifier, decode_hex, router};
use crate::events::{BoxFuture, EntityMetadata, EventSource, ScheduledEvent};

/// Scheduled events for /meetup, or an error.
struct Events(Result<Vec<ScheduledEvent>, String>);

impl EventSource for Events {
    fn scheduled_events(&self) -> BoxFuture<'_, Vec<ScheduledEvent>> {
        let result = self.0.clone().map_err(lambda_http::Error::from);
        Box::pin(async move { result })
    }
}

fn state(verifier: Verifier, events: Result<Vec<ScheduledEvent>, String>) -> AppState {
    AppState {
        verifier: std::sync::Arc::new(verifier),
        events: std::sync::Arc::new(Events(events)),
        guild_id: "9".into(),
    }
}

const TIMESTAMP: &str = "1791400000";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

struct Signer {
    key: Ed25519KeyPair,
}

impl Signer {
    fn new() -> Self {
        Self {
            key: Ed25519KeyPair::generate().unwrap(),
        }
    }

    fn verifier(&self) -> Verifier {
        Verifier::from_hex(&hex(self.key.public_key().as_ref())).unwrap()
    }

    fn sign(&self, body: &str) -> String {
        hex(self
            .key
            .sign(format!("{TIMESTAMP}{body}").as_bytes())
            .as_ref())
    }
}

async fn post(
    verifier: Verifier,
    path: &str,
    body: &str,
    signature: Option<&str>,
) -> (StatusCode, Option<Value>) {
    send(state(verifier, Ok(vec![])), path, body, signature).await
}

async fn send(
    state: AppState,
    path: &str,
    body: &str,
    signature: Option<&str>,
) -> (StatusCode, Option<Value>) {
    let mut request = Request::post(path).header("content-type", "application/json");
    if let Some(signature) = signature {
        request = request
            .header("x-signature-ed25519", signature)
            .header("x-signature-timestamp", TIMESTAMP);
    }
    let response = router(state)
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).ok())
}

async fn interaction(signer: &Signer, body: &str) -> (StatusCode, Option<Value>) {
    let signature = signer.sign(body);
    post(
        signer.verifier(),
        "/api/discord/interactions",
        body,
        Some(&signature),
    )
    .await
}

#[tokio::test]
async fn answers_ping_with_pong() {
    let (status, body) = interaction(&Signer::new(), r#"{"type":1}"#).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, Some(json!({ "type": 1 })));
}

#[tokio::test]
async fn ping_command_replies_privately() {
    let body = r#"{"type":2,"data":{"name":"ping"}}"#;
    let (status, body) = interaction(&Signer::new(), body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        Some(json!({ "type": 4, "data": { "content": "Pong!", "flags": 64 } }))
    );
}

#[tokio::test]
async fn unknown_command_replies_privately() {
    let body = r#"{"type":2,"data":{"name":"nope"}}"#;
    let (_, body) = interaction(&Signer::new(), body).await;
    assert_eq!(
        body,
        Some(json!({
            "type": 4,
            "data": { "content": "Sorry, I don't know that command yet.", "flags": 64 },
        }))
    );
}

#[tokio::test]
async fn unsupported_interaction_type_is_rejected() {
    let (status, _) = interaction(&Signer::new(), r#"{"type":3}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn signed_invalid_json_is_rejected() {
    let (status, _) = interaction(&Signer::new(), "not json").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn rejects_bad_signatures() {
    let signer = Signer::new();
    let body = r#"{"type":1}"#;
    let valid = signer.sign(body);
    let cases = [
        ("missing", None),
        ("other key", Some(Signer::new().sign(body))),
        ("other body", Some(signer.sign(r#"{"type":2}"#))),
        ("not hex", Some("zz".repeat(64))),
        ("odd length", Some(valid[1..].to_owned())),
        ("truncated", Some(valid[..64].to_owned())),
    ];
    for (name, signature) in cases {
        let (status, body) = post(
            signer.verifier(),
            "/api/discord/interactions",
            body,
            signature.as_deref(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{name}");
        assert_eq!(
            body,
            Some(json!({ "error": "invalid request signature" })),
            "{name}"
        );
    }
}

#[tokio::test]
async fn webhook_ping_and_events_return_no_content() {
    let signer = Signer::new();
    for body in [
        r#"{"version":1,"application_id":"1","type":0}"#,
        r#"{"version":1,"application_id":"1","type":1,"event":{"type":"APPLICATION_AUTHORIZED","timestamp":"2026-10-07T00:00:00Z","data":{}}}"#,
    ] {
        let signature = signer.sign(body);
        let (status, _) = post(
            signer.verifier(),
            "/api/discord/events",
            body,
            Some(&signature),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    }
}

#[tokio::test]
async fn webhook_event_with_bad_signature_is_unauthorized() {
    let signer = Signer::new();
    let body = r#"{"version":1,"application_id":"1","type":0}"#;
    let signature = Signer::new().sign(body);
    let (status, _) = post(
        signer.verifier(),
        "/api/discord/events",
        body,
        Some(&signature),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn signed_invalid_webhook_event_is_rejected() {
    let signer = Signer::new();
    let signature = signer.sign("{}");
    let (status, _) = post(
        signer.verifier(),
        "/api/discord/events",
        "{}",
        Some(&signature),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unknown_path_is_not_found() {
    let (status, _) = post(Signer::new().verifier(), "/api/discord/other", "{}", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[test]
fn public_key_must_be_32_bytes_of_hex() {
    let valid = "ab".repeat(32);
    assert!(Verifier::from_hex(&valid).is_some());
    assert!(Verifier::from_hex(&"ab".repeat(31)).is_none());
    assert!(Verifier::from_hex(&"zz".repeat(32)).is_none());
    assert!(Verifier::from_hex("").is_none());
}

#[test]
fn decode_hex_handles_edge_cases() {
    assert_eq!(decode_hex(""), Some(vec![]));
    assert_eq!(decode_hex("00ff"), Some(vec![0, 255]));
    assert_eq!(decode_hex("0"), None);
    assert_eq!(decode_hex("é0"), None);
}

fn meetup_event(id: &str, start: &str, status: u8) -> ScheduledEvent {
    ScheduledEvent {
        id: id.into(),
        name: "AI Night".into(),
        description: None,
        scheduled_start_time: start.into(),
        status,
        entity_type: 3,
        channel_id: None,
        entity_metadata: Some(EntityMetadata {
            location: Some("Fairfield Library".into()),
        }),
    }
}

async fn meetup_command(events: Result<Vec<ScheduledEvent>, String>) -> Value {
    let signer = Signer::new();
    let body = r#"{"type":2,"data":{"name":"meetup"}}"#;
    let signature = signer.sign(body);
    let (status, response) = send(
        state(signer.verifier(), events),
        "/api/discord/interactions",
        body,
        Some(&signature),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    response.unwrap()
}

#[tokio::test]
async fn meetup_shows_the_next_event_to_the_channel() {
    let events = vec![
        meetup_event("2", "2099-01-02T00:00:00Z", 1),
        meetup_event("1", "2099-01-01T00:00:00Z", 1),
    ];
    let response = meetup_command(Ok(events)).await;
    assert_eq!(response["type"], 4);
    assert_eq!(
        response["data"]["flags"],
        Value::Null,
        "visible to the channel"
    );
    assert_eq!(response["data"]["allowed_mentions"], json!({ "parse": [] }));
    let content = response["data"]["content"].as_str().unwrap();
    assert!(
        content.starts_with("**Next meetup: AI Night**\n<t:4070908800:F>"),
        "{content}"
    );
    assert!(
        content.ends_with("https://discord.com/events/9/1"),
        "{content}"
    );
}

#[tokio::test]
async fn meetup_says_when_one_is_happening_now() {
    let response = meetup_command(Ok(vec![meetup_event("5", "2020-01-01T00:00:00Z", 2)])).await;
    let content = response["data"]["content"].as_str().unwrap();
    assert!(
        content.starts_with("**Happening now: AI Night**"),
        "{content}"
    );
}

#[tokio::test]
async fn meetup_without_events_replies_privately() {
    let past = meetup_event("1", "2020-01-01T00:00:00Z", 1);
    let response = meetup_command(Ok(vec![past])).await;
    assert_eq!(response["data"]["flags"], 64);
    assert_eq!(
        response["data"]["content"],
        "No meetups are scheduled yet. Watch #announcements for the next one."
    );
}

#[tokio::test]
async fn meetup_failure_replies_privately() {
    let response = meetup_command(Err("discord down".into())).await;
    assert_eq!(response["data"]["flags"], 64);
    let content = response["data"]["content"].as_str().unwrap();
    assert!(
        content.starts_with("Sorry, I couldn't load meetups"),
        "{content}"
    );
}
