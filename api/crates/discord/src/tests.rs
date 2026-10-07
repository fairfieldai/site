use std::fmt::Write as _;

use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{Verifier, decode_hex, router};

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
    let mut request = Request::post(path).header("content-type", "application/json");
    if let Some(signature) = signature {
        request = request
            .header("x-signature-ed25519", signature)
            .header("x-signature-timestamp", TIMESTAMP);
    }
    let response = router(verifier)
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
