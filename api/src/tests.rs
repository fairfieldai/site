use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{AppState, router};
use crate::auth::{AuthError, KeySource, Verifier};

const ISSUER: &str = "https://cognito-idp.us-east-1.amazonaws.com/us-east-1_test";
const CLIENT_ID: &str = "test-client";
const SIGNING_KEY: &[u8] = include_bytes!("../testdata/signing-key.pem");
const OTHER_KEY: &[u8] = include_bytes!("../testdata/other-key.pem");
const JWKS: &str = include_str!("../testdata/jwks.json");

/// Serves the fixture JWKS, or fails, and counts fetches.
struct FixtureKeys {
    fetches: Arc<AtomicUsize>,
    available: bool,
}

impl KeySource for FixtureKeys {
    fn fetch(&self) -> Pin<Box<dyn Future<Output = Result<JwkSet, AuthError>> + Send + '_>> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        let result = if self.available {
            Ok(serde_json::from_str(JWKS).unwrap())
        } else {
            Err(AuthError::Unavailable)
        };
        Box::pin(async move { result })
    }
}

fn state(available: bool) -> (AppState, Arc<AtomicUsize>) {
    let fetches = Arc::new(AtomicUsize::new(0));
    let keys = FixtureKeys {
        fetches: Arc::clone(&fetches),
        available,
    };
    let verifier = Verifier::new(ISSUER.into(), CLIENT_ID.into(), Box::new(keys));
    (
        AppState {
            verifier: Arc::new(verifier),
        },
        fetches,
    )
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn claims() -> Value {
    json!({
        "sub": "user-123",
        "username": "user-123",
        "iss": ISSUER,
        "client_id": CLIENT_ID,
        "token_use": "access",
        "exp": now() + 600,
    })
}

fn sign(claims: &Value, kid: &str, pem: &[u8]) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.into());
    encode(&header, claims, &EncodingKey::from_rsa_pem(pem).unwrap()).unwrap()
}

fn token_with(change: impl FnOnce(&mut Value)) -> String {
    let mut claims = claims();
    change(&mut claims);
    sign(&claims, "test-key", SIGNING_KEY)
}

async fn send(
    state: AppState,
    method: Method,
    uri: &str,
    authorization: Option<&str>,
) -> (StatusCode, Option<Value>) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(value) = authorization {
        request = request.header(header::AUTHORIZATION, value);
    }
    let response = router(state)
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).ok())
}

async fn me(token: &str) -> (StatusCode, Option<Value>) {
    let auth = format!("Bearer {token}");
    send(state(true).0, Method::GET, "/api/me", Some(&auth)).await
}

#[tokio::test]
async fn health_returns_ok() {
    let (status, body) = send(state(true).0, Method::GET, "/api/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, Some(json!({ "status": "ok" })));
}

#[tokio::test]
async fn unknown_route_returns_json_404() {
    let (status, body) = send(state(true).0, Method::GET, "/api/missing", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, Some(json!({ "error": "not found" })));
}

#[tokio::test]
async fn routes_outside_api_prefix_are_not_found() {
    let (status, _) = send(state(true).0, Method::GET, "/health", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn wrong_method_is_rejected() {
    let (status, _) = send(state(true).0, Method::POST, "/api/health", None).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn me_returns_the_signed_in_user() {
    let (status, body) = me(&token_with(|_| {})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        Some(json!({ "sub": "user-123", "username": "user-123" }))
    );
}

#[tokio::test]
async fn me_without_token_is_unauthorized() {
    let (status, body) = send(state(true).0, Method::GET, "/api/me", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, Some(json!({ "error": "unauthorized" })));
}

#[tokio::test]
async fn non_bearer_authorization_is_unauthorized() {
    let token = token_with(|_| {});
    let basic = format!("Basic {token}");
    let (status, _) = send(state(true).0, Method::GET, "/api/me", Some(&basic)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unauthorized_response_asks_for_a_bearer_token() {
    let response = router(state(true).0)
        .oneshot(Request::get("/api/me").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers()[header::WWW_AUTHENTICATE], "Bearer");
}

#[tokio::test]
async fn rejects_invalid_tokens() {
    let cases = [
        ("malformed", "not-a-jwt".to_owned()),
        ("expired", token_with(|c| c["exp"] = json!(now() - 600))),
        (
            "other issuer",
            token_with(|c| c["iss"] = json!("https://cognito-idp.us-east-1.amazonaws.com/other")),
        ),
        ("id token", token_with(|c| c["token_use"] = json!("id"))),
        (
            "other client",
            token_with(|c| c["client_id"] = json!("other-client")),
        ),
        ("missing exp", token_with(|c| c["exp"] = Value::Null)),
        ("wrong key", sign(&claims(), "test-key", OTHER_KEY)),
        ("unknown kid", sign(&claims(), "unknown-key", SIGNING_KEY)),
    ];
    for (name, token) in cases {
        let (status, _) = me(&token).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{name}");
    }
}

#[tokio::test]
async fn rejects_tokens_with_another_algorithm() {
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("test-key".into());
    let token = encode(&header, &claims(), &EncodingKey::from_secret(b"secret")).unwrap();
    let (status, _) = me(&token).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn caches_keys_and_limits_refetches_for_unknown_kids() {
    let (state, fetches) = state(true);
    let verifier = &state.verifier;

    assert!(verifier.verify(&token_with(|_| {})).await.is_ok());
    assert!(verifier.verify(&token_with(|_| {})).await.is_ok());
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    let unknown = sign(&claims(), "unknown-key", SIGNING_KEY);
    assert_eq!(verifier.verify(&unknown).await, Err(AuthError::Invalid));
    assert_eq!(verifier.verify(&unknown).await, Err(AuthError::Invalid));
    assert_eq!(fetches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn first_unknown_kid_triggers_a_fetch() {
    let (state, fetches) = state(true);
    let unknown = sign(&claims(), "unknown-key", SIGNING_KEY);
    assert_eq!(
        state.verifier.verify(&unknown).await,
        Err(AuthError::Invalid)
    );
    assert_eq!(fetches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn key_outage_is_service_unavailable() {
    let (state, _) = state(false);
    let auth = format!("Bearer {}", token_with(|_| {}));
    let (status, body) = send(state, Method::GET, "/api/me", Some(&auth)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body, Some(json!({ "error": "authentication unavailable" })));
}

#[tokio::test]
async fn retries_key_fetch_after_an_outage() {
    let (state, fetches) = state(false);
    let token = token_with(|_| {});
    assert_eq!(
        state.verifier.verify(&token).await,
        Err(AuthError::Unavailable)
    );
    assert_eq!(
        state.verifier.verify(&token).await,
        Err(AuthError::Unavailable)
    );
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}
