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

use shared::events::MemoryEvents;
use shared::subscribers::MemorySubscribers;

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
            discord: None,
            events: Arc::new(MemoryEvents::default()),
            subscribers: Arc::new(MemorySubscribers::default()),
            site_url: "https://www.fairfieldct.ai".into(),
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

mod discord_linking {
    use std::sync::Mutex;

    use shared::links::{Link, MemoryLinks};

    use super::*;
    use crate::discord_link::{
        DiscordLinking, DiscordOAuth, DiscordUser, OAuthError, Tokens, role_connection,
    };

    type OAuthFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, OAuthError>> + Send + 'a>>;

    /// What a fake Discord call returns.
    #[derive(Clone, Copy, PartialEq)]
    enum Outcome {
        Ok,
        InvalidGrant,
        Fail,
    }

    impl Outcome {
        fn result<T>(self, value: T) -> Result<T, OAuthError> {
            match self {
                Self::Ok => Ok(value),
                Self::InvalidGrant => Err(OAuthError::InvalidGrant),
                Self::Fail => Err(OAuthError::Failed("discord down".into())),
            }
        }
    }

    /// Records Discord calls and answers them as configured.
    struct FakeDiscord {
        calls: Mutex<Vec<String>>,
        exchange: Outcome,
        scope: &'static str,
        user: (&'static str, &'static str),
        refresh: Outcome,
        set_member: Outcome,
    }

    impl Default for FakeDiscord {
        fn default() -> Self {
            Self {
                calls: Mutex::default(),
                exchange: Outcome::Ok,
                scope: "identify role_connections.write",
                user: ("d1", "alice"),
                refresh: Outcome::Ok,
                set_member: Outcome::Ok,
            }
        }
    }

    impl FakeDiscord {
        fn record(&self, call: String) {
            self.calls.lock().unwrap().push(call);
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn tokens(&self, refresh_token: &str) -> Tokens {
            Tokens {
                access_token: format!("access-for-{refresh_token}"),
                refresh_token: refresh_token.into(),
                scope: self.scope.into(),
            }
        }
    }

    impl DiscordOAuth for FakeDiscord {
        fn exchange_code<'a>(&'a self, code: &'a str) -> OAuthFuture<'a, Tokens> {
            self.record(format!("exchange {code}"));
            let result = self.exchange.result(self.tokens("new-refresh"));
            Box::pin(async move { result })
        }
        fn refresh<'a>(&'a self, refresh_token: &'a str) -> OAuthFuture<'a, Tokens> {
            self.record(format!("refresh {refresh_token}"));
            let result = self
                .refresh
                .result(self.tokens(&format!("{refresh_token}-rotated")));
            Box::pin(async move { result })
        }
        fn current_user<'a>(&'a self, access_token: &'a str) -> OAuthFuture<'a, DiscordUser> {
            self.record(format!("user {access_token}"));
            let (id, username) = self.user;
            Box::pin(async move {
                Ok(DiscordUser {
                    id: id.into(),
                    username: username.into(),
                })
            })
        }
        fn set_member<'a>(&'a self, access_token: &'a str, member: bool) -> OAuthFuture<'a, ()> {
            self.record(format!("set_member {member} {access_token}"));
            let result = self.set_member.result(());
            Box::pin(async move { result })
        }
        fn revoke<'a>(&'a self, refresh_token: &'a str) -> OAuthFuture<'a, ()> {
            self.record(format!("revoke {refresh_token}"));
            Box::pin(async { Ok(()) })
        }
    }

    fn link(user: &str, discord: &str, refresh: &str) -> Link {
        Link {
            user_id: user.into(),
            discord_id: discord.into(),
            discord_username: format!("name-{discord}"),
            refresh_token: refresh.into(),
            linked_at: 1,
        }
    }

    struct Harness {
        discord: Arc<FakeDiscord>,
        links: Arc<MemoryLinks>,
        state: AppState,
    }

    fn harness(discord: FakeDiscord, links: Vec<Link>, manage_role_connection: bool) -> Harness {
        let discord = Arc::new(discord);
        let links = Arc::new(MemoryLinks::with(links));
        let (mut state, _) = state(true);
        state.discord = Some(Arc::new(DiscordLinking {
            oauth: discord.clone(),
            links: links.clone(),
            manage_role_connection,
        }));
        Harness {
            discord,
            links,
            state,
        }
    }

    async fn call(
        state: AppState,
        method: Method,
        body: Option<Value>,
    ) -> (StatusCode, Option<Value>) {
        let mut request = Request::builder()
            .method(method)
            .uri("/api/account/discord")
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", token_with(|_| {})),
            );
        let body = match body {
            Some(body) => {
                request = request.header(header::CONTENT_TYPE, "application/json");
                Body::from(body.to_string())
            }
            None => Body::empty(),
        };
        let response = router(state)
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).ok())
    }

    async fn connect(h: &Harness) -> (StatusCode, Option<Value>) {
        call(
            h.state.clone(),
            Method::POST,
            Some(json!({ "code": "abc" })),
        )
        .await
    }

    async fn disconnect(h: &Harness) -> StatusCode {
        call(h.state.clone(), Method::DELETE, None).await.0
    }

    #[tokio::test]
    async fn unavailable_without_a_discord_application() {
        let (status, body) = call(state(true).0, Method::GET, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body.unwrap()["error"],
            "Discord linking isn't available here"
        );
    }

    #[tokio::test]
    async fn requires_sign_in() {
        let h = harness(FakeDiscord::default(), vec![], true);
        let response = router(h.state)
            .oneshot(
                Request::get("/api/account/discord")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn status_reports_the_link_and_scopes() {
        let h = harness(
            FakeDiscord::default(),
            vec![link("user-123", "d1", "r")],
            true,
        );
        let (status, body) = call(h.state.clone(), Method::GET, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            Some(
                json!({ "linked": true, "username": "name-d1", "scopes": "identify role_connections.write" })
            )
        );
        let dev = harness(FakeDiscord::default(), vec![], false);
        let (_, body) = call(dev.state.clone(), Method::GET, None).await;
        assert_eq!(body, Some(json!({ "linked": false, "scopes": "identify" })));
    }

    #[tokio::test]
    async fn connecting_in_prod_links_and_sets_the_role_connection() {
        let h = harness(FakeDiscord::default(), vec![], true);
        let (status, body) = connect(&h).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.unwrap()["username"], "alice");
        assert_eq!(
            h.discord.calls(),
            [
                "exchange abc",
                "user access-for-new-refresh",
                "set_member true access-for-new-refresh"
            ]
        );
        let saved = h.links.all();
        assert_eq!(saved.len(), 1);
        assert_eq!(
            (
                saved[0].user_id.as_str(),
                saved[0].discord_id.as_str(),
                saved[0].refresh_token.as_str()
            ),
            ("user-123", "d1", "new-refresh")
        );
    }

    #[tokio::test]
    async fn connecting_in_dev_only_records_the_link() {
        let discord = FakeDiscord {
            scope: "identify",
            ..FakeDiscord::default()
        };
        let h = harness(discord, vec![], false);
        assert_eq!(connect(&h).await.0, StatusCode::OK);
        assert_eq!(
            h.discord.calls(),
            ["exchange abc", "user access-for-new-refresh"]
        );
        assert_eq!(
            h.links.all()[0].refresh_token,
            "",
            "dev keeps no Discord credentials"
        );
    }

    #[tokio::test]
    async fn expired_codes_are_rejected() {
        let h = harness(
            FakeDiscord {
                exchange: Outcome::InvalidGrant,
                ..FakeDiscord::default()
            },
            vec![],
            true,
        );
        assert_eq!(connect(&h).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(h.links.all(), Vec::<Link>::new());
    }

    #[tokio::test]
    async fn discord_outages_are_a_bad_gateway() {
        let h = harness(
            FakeDiscord {
                exchange: Outcome::Fail,
                ..FakeDiscord::default()
            },
            vec![],
            true,
        );
        assert_eq!(connect(&h).await.0, StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn prod_requires_the_role_connection_scope() {
        let h = harness(
            FakeDiscord {
                scope: "identify",
                ..FakeDiscord::default()
            },
            vec![],
            true,
        );
        assert_eq!(connect(&h).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(h.links.all(), Vec::<Link>::new());
        assert!(
            !h.discord
                .calls()
                .iter()
                .any(|c| c.starts_with("set_member"))
        );
    }

    #[tokio::test]
    async fn a_discord_account_linked_elsewhere_is_a_conflict() {
        let h = harness(
            FakeDiscord::default(),
            vec![link("someone-else", "d1", "r")],
            true,
        );
        assert_eq!(connect(&h).await.0, StatusCode::CONFLICT);
        assert_eq!(h.links.all(), [link("someone-else", "d1", "r")]);
        assert!(
            !h.discord
                .calls()
                .iter()
                .any(|c| c.starts_with("set_member"))
        );
    }

    #[tokio::test]
    async fn a_failed_role_connection_undoes_the_link() {
        let h = harness(
            FakeDiscord {
                set_member: Outcome::Fail,
                ..FakeDiscord::default()
            },
            vec![],
            true,
        );
        assert_eq!(connect(&h).await.0, StatusCode::BAD_GATEWAY);
        assert_eq!(h.links.all(), Vec::<Link>::new());
    }

    #[tokio::test]
    async fn switching_discord_accounts_clears_the_old_role_connection() {
        let h = harness(
            FakeDiscord::default(),
            vec![link("user-123", "d0", "old")],
            true,
        );
        assert_eq!(connect(&h).await.0, StatusCode::OK);
        assert_eq!(
            h.discord.calls()[3..],
            [
                "refresh old",
                "set_member false access-for-old-rotated",
                "revoke old-rotated"
            ]
        );
        assert_eq!(h.links.all()[0].discord_id, "d1");
    }

    #[tokio::test]
    async fn disconnecting_in_prod_clears_the_role_and_revokes() {
        let h = harness(
            FakeDiscord::default(),
            vec![link("user-123", "d1", "r1")],
            true,
        );
        assert_eq!(disconnect(&h).await, StatusCode::NO_CONTENT);
        assert_eq!(
            h.discord.calls(),
            [
                "refresh r1",
                "set_member false access-for-r1-rotated",
                "revoke r1-rotated"
            ]
        );
        assert_eq!(h.links.all(), Vec::<Link>::new());
    }

    #[tokio::test]
    async fn disconnecting_keeps_the_rotated_token_when_discord_fails() {
        let h = harness(
            FakeDiscord {
                set_member: Outcome::Fail,
                ..FakeDiscord::default()
            },
            vec![link("user-123", "d1", "r1")],
            true,
        );
        assert_eq!(disconnect(&h).await, StatusCode::BAD_GATEWAY);
        assert_eq!(h.links.all()[0].refresh_token, "r1-rotated");
    }

    #[tokio::test]
    async fn disconnecting_after_removing_the_app_in_discord_still_unlinks() {
        let h = harness(
            FakeDiscord {
                refresh: Outcome::InvalidGrant,
                ..FakeDiscord::default()
            },
            vec![link("user-123", "d1", "r1")],
            true,
        );
        assert_eq!(disconnect(&h).await, StatusCode::NO_CONTENT);
        assert_eq!(h.discord.calls(), ["refresh r1"]);
        assert_eq!(h.links.all(), Vec::<Link>::new());
    }

    #[tokio::test]
    async fn disconnecting_during_a_discord_outage_keeps_the_link() {
        let h = harness(
            FakeDiscord {
                refresh: Outcome::Fail,
                ..FakeDiscord::default()
            },
            vec![link("user-123", "d1", "r1")],
            true,
        );
        assert_eq!(disconnect(&h).await, StatusCode::BAD_GATEWAY);
        assert_eq!(h.links.all().len(), 1);
    }

    #[tokio::test]
    async fn disconnecting_in_dev_never_calls_discord() {
        let h = harness(
            FakeDiscord::default(),
            vec![link("user-123", "d1", "")],
            false,
        );
        assert_eq!(disconnect(&h).await, StatusCode::NO_CONTENT);
        assert_eq!(h.discord.calls(), Vec::<String>::new());
        assert_eq!(h.links.all(), Vec::<Link>::new());
    }

    #[tokio::test]
    async fn disconnecting_without_a_link_is_fine() {
        let h = harness(FakeDiscord::default(), vec![], true);
        assert_eq!(disconnect(&h).await, StatusCode::NO_CONTENT);
        assert_eq!(h.discord.calls(), Vec::<String>::new());
    }

    #[test]
    fn role_connection_uses_string_booleans() {
        assert_eq!(
            role_connection(true),
            json!({ "platform_name": "fairfieldct.ai", "metadata": { "member": "1" } })
        );
        assert_eq!(role_connection(false)["metadata"]["member"], "0");
    }
}

/// Sends a request, signed in when `signed_in`, with an optional JSON body.
async fn request(
    state: AppState,
    method: Method,
    uri: &str,
    signed_in: bool,
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut request = Request::builder().method(method).uri(uri);
    if signed_in {
        request = request.header(
            header::AUTHORIZATION,
            format!("Bearer {}", token_with(|_| {})),
        );
    }
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let response = router(state)
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, bytes.to_vec())
}

async fn json_request(
    state: AppState,
    method: Method,
    uri: &str,
    signed_in: bool,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, _, bytes) = request(state, method, uri, signed_in, body).await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A store that fails every call, for error paths.
struct Broken;

fn broken<T>() -> shared::links::BoxFuture<'static, Result<T, shared::Error>> {
    Box::pin(async { Err("store down".into()) })
}

mod events {
    use shared::events::{Attendee, Event, EventStore, RsvpError, Status};
    use shared::links::BoxFuture;
    use shared::time;

    use super::*;

    impl EventStore for Broken {
        fn list(&self) -> BoxFuture<'_, Result<Vec<Event>, shared::Error>> {
            broken()
        }
        fn get<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<Option<Event>, shared::Error>> {
            broken()
        }
        fn save<'a>(&'a self, _: &'a Event) -> BoxFuture<'a, Result<(), shared::Error>> {
            broken()
        }
        fn close<'a>(
            &'a self,
            _: &'a str,
            _: Status,
        ) -> BoxFuture<'a, Result<bool, shared::Error>> {
            broken()
        }
        fn rsvp<'a>(
            &'a self,
            _: &'a str,
            _: &'a Attendee,
            _: i64,
        ) -> BoxFuture<'a, Result<u32, RsvpError>> {
            Box::pin(async { Err(RsvpError::Store("store down".into())) })
        }
        fn cancel_rsvp<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
        ) -> BoxFuture<'a, Result<u32, RsvpError>> {
            Box::pin(async { Err(RsvpError::Store("store down".into())) })
        }
        fn rsvps_by_user<'a>(
            &'a self,
            _: &'a str,
        ) -> BoxFuture<'a, Result<Vec<String>, shared::Error>> {
            broken()
        }
        fn attendees<'a>(
            &'a self,
            _: &'a str,
        ) -> BoxFuture<'a, Result<Vec<Attendee>, shared::Error>> {
            broken()
        }
    }

    /// An event starting `offset` seconds from now and lasting two hours.
    fn event(id: &str, offset: i64, status: Status) -> Event {
        let starts_at = time::now() + offset;
        Event {
            id: id.into(),
            name: format!("Meetup {id}"),
            description: String::new(),
            starts_at,
            ends_at: starts_at + 7200,
            location: "Fairfield Library".into(),
            url: format!("https://discord.com/events/9/{id}"),
            status,
            rsvps: 0,
        }
    }

    fn with_events(events: Vec<Event>) -> AppState {
        let (mut state, _) = state(true);
        state.events = Arc::new(MemoryEvents::with(events));
        state
    }

    fn broken_state() -> AppState {
        let (mut state, _) = state(true);
        state.events = Arc::new(Broken);
        state
    }

    const DAY: i64 = 86_400;

    #[tokio::test]
    async fn lists_upcoming_then_past_events_without_signing_in() {
        let state = with_events(vec![
            event("1", 7 * DAY, Status::Scheduled),
            event("2", DAY, Status::Scheduled),
            event("3", -30 * DAY, Status::Ended),
            event("4", -2 * DAY, Status::Scheduled),
            event("5", 3 * DAY, Status::Canceled),
            event("6", -3 * DAY, Status::Canceled),
            event("7", -3600, Status::Active),
        ]);
        let (status, body) = json_request(state, Method::GET, "/api/events", false, None).await;
        assert_eq!(status, StatusCode::OK);
        let listed: Vec<(String, bool, String)> = body["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["id"].as_str().unwrap().to_owned(),
                    e["upcoming"].as_bool().unwrap(),
                    e["status"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        let expected = [
            ("7", true, "active"),
            ("2", true, "scheduled"),
            ("5", true, "canceled"),
            ("1", true, "scheduled"),
            ("4", false, "scheduled"),
            ("3", false, "ended"),
        ]
        .map(|(id, upcoming, status)| (id.to_owned(), upcoming, status.to_owned()));
        assert_eq!(listed, expected);
        let first = &body["events"][0];
        for field in [
            "name",
            "description",
            "starts_at",
            "ends_at",
            "location",
            "url",
            "rsvps",
        ] {
            assert!(!first[field].is_null(), "{field}");
        }
    }

    #[tokio::test]
    async fn calendar_feed_is_public() {
        let state = with_events(vec![
            event("2", DAY, Status::Scheduled),
            event("1", -DAY, Status::Ended),
        ]);
        let (status, headers, body) =
            request(state, Method::GET, "/api/events.ics", false, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers[header::CONTENT_TYPE],
            "text/calendar; charset=utf-8"
        );
        let body = String::from_utf8(body).unwrap();
        assert!(body.starts_with("BEGIN:VCALENDAR\r\n"));
        let first = body.find("UID:1@").unwrap();
        let second = body.find("UID:2@").unwrap();
        assert!(first < second, "oldest first");
        assert!(body.contains("URL:https://www.fairfieldct.ai/events/#event-2\r\n"));
    }

    #[tokio::test]
    async fn rsvp_requires_sign_in() {
        let state = with_events(vec![event("2", DAY, Status::Scheduled)]);
        for method in [Method::PUT, Method::DELETE] {
            let (status, _) =
                json_request(state.clone(), method, "/api/events/2/rsvp", false, None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
        let (status, _) = json_request(state, Method::GET, "/api/account/rsvps", false, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn members_rsvp_and_change_their_minds() {
        let state = with_events(vec![event("2", DAY, Status::Scheduled)]);
        let rsvp = |method| json_request(state.clone(), method, "/api/events/2/rsvp", true, None);

        assert_eq!(
            rsvp(Method::PUT).await,
            (StatusCode::OK, json!({ "going": true, "rsvps": 1 }))
        );
        assert_eq!(
            rsvp(Method::PUT).await,
            (StatusCode::OK, json!({ "going": true, "rsvps": 1 }))
        );
        let (_, mine) =
            json_request(state.clone(), Method::GET, "/api/account/rsvps", true, None).await;
        assert_eq!(mine, json!({ "events": ["2"] }));
        let attendees = state.events.attendees("2").await.unwrap();
        assert_eq!(
            attendees,
            [Attendee {
                user_id: "user-123".into(),
                username: "user-123".into()
            }]
        );

        assert_eq!(
            rsvp(Method::DELETE).await,
            (StatusCode::OK, json!({ "going": false, "rsvps": 0 }))
        );
        assert_eq!(
            rsvp(Method::DELETE).await,
            (StatusCode::OK, json!({ "going": false, "rsvps": 0 }))
        );
        let (_, mine) = json_request(state, Method::GET, "/api/account/rsvps", true, None).await;
        assert_eq!(mine, json!({ "events": [] }));
    }

    #[tokio::test]
    async fn rsvps_need_an_upcoming_event() {
        let state = with_events(vec![
            event("3", -DAY, Status::Ended),
            event("5", DAY, Status::Canceled),
        ]);
        for (uri, expected) in [
            ("/api/events/3/rsvp", StatusCode::CONFLICT),
            ("/api/events/5/rsvp", StatusCode::CONFLICT),
            ("/api/events/404/rsvp", StatusCode::NOT_FOUND),
            ("/api/events/not-an-id/rsvp", StatusCode::NOT_FOUND),
            (
                "/api/events/123456789012345678901/rsvp",
                StatusCode::NOT_FOUND,
            ),
        ] {
            let (status, body) = json_request(state.clone(), Method::PUT, uri, true, None).await;
            assert_eq!(status, expected, "{uri}");
            assert!(body["error"].is_string(), "{uri}");
        }
        let (status, _) = json_request(
            state,
            Method::DELETE,
            "/api/events/not-an-id/rsvp",
            true,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn events_that_just_ended_are_past() {
        let now = time::now();
        let mut ending = event("1", -7200, Status::Active);
        ending.ends_at = now;
        let mut almost = event("2", -7200, Status::Active);
        almost.ends_at = now + 1;
        let arranged = crate::events::arrange(vec![ending, almost], now);
        let placed: Vec<(&str, bool)> = arranged
            .iter()
            .map(|(e, up)| (e.id.as_str(), *up))
            .collect();
        assert_eq!(placed, [("2", true), ("1", false)]);
    }

    #[tokio::test]
    async fn malformed_ids_never_reach_the_store() {
        for uri in [
            "/api/events/not-an-id/rsvp",
            "/api/events/12a/rsvp",
            "/api/events/123456789012345678901/rsvp",
        ] {
            for method in [Method::PUT, Method::DELETE] {
                let (status, _) = json_request(broken_state(), method, uri, true, None).await;
                assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
            }
        }
        // The longest valid snowflake does reach it.
        let (status, _) = json_request(
            broken_state(),
            Method::PUT,
            "/api/events/12345678901234567890/rsvp",
            true,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn store_failures_are_server_errors() {
        for (method, uri, signed_in) in [
            (Method::GET, "/api/events", false),
            (Method::GET, "/api/events.ics", false),
            (Method::PUT, "/api/events/2/rsvp", true),
            (Method::DELETE, "/api/events/2/rsvp", true),
            (Method::GET, "/api/account/rsvps", true),
        ] {
            let (status, body) = json_request(broken_state(), method, uri, signed_in, None).await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{uri}");
            assert_eq!(body, json!({ "error": "couldn't load events" }), "{uri}");
        }
    }
}

mod email {
    use shared::links::BoxFuture;
    use shared::subscribers::{Subscriber, Subscribers};

    use super::*;

    impl Subscribers for Broken {
        fn get<'a>(
            &'a self,
            _: &'a str,
        ) -> BoxFuture<'a, Result<Option<Subscriber>, shared::Error>> {
            broken()
        }
        fn subscribe(&self, _: Subscriber) -> BoxFuture<'_, Result<(), shared::Error>> {
            broken()
        }
        fn unsubscribe<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<bool, shared::Error>> {
            broken()
        }
        fn unsubscribe_token<'a>(
            &'a self,
            _: &'a str,
        ) -> BoxFuture<'a, Result<bool, shared::Error>> {
            broken()
        }
        fn list(&self) -> BoxFuture<'_, Result<Vec<Subscriber>, shared::Error>> {
            broken()
        }
    }

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn tokens_are_random_hex() {
        let token = crate::email::new_token().unwrap();
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(token, crate::email::new_token().unwrap());
    }

    #[tokio::test]
    async fn preferences_require_sign_in() {
        for method in [Method::GET, Method::PUT] {
            let (status, _) = json_request(
                state(true).0,
                method,
                "/api/account/email",
                false,
                Some(json!({ "announcements": true })),
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn members_opt_in_and_out() {
        let (state, _) = state(true);
        let get = || json_request(state.clone(), Method::GET, "/api/account/email", true, None);
        let put = |on: bool| {
            json_request(
                state.clone(),
                Method::PUT,
                "/api/account/email",
                true,
                Some(json!({ "announcements": on })),
            )
        };

        assert_eq!(
            get().await,
            (StatusCode::OK, json!({ "announcements": false }))
        );
        assert_eq!(
            put(true).await,
            (StatusCode::OK, json!({ "announcements": true }))
        );
        let subscriber = state.subscribers.get("user-123").await.unwrap().unwrap();
        assert_eq!(subscriber.username, "user-123");
        assert_eq!(subscriber.token.len(), 64);
        // Opting in again keeps the first token.
        put(true).await;
        assert_eq!(state.subscribers.list().await.unwrap(), [subscriber]);
        assert_eq!(
            get().await,
            (StatusCode::OK, json!({ "announcements": true }))
        );
        assert_eq!(
            put(false).await,
            (StatusCode::OK, json!({ "announcements": false }))
        );
        assert_eq!(
            put(false).await,
            (StatusCode::OK, json!({ "announcements": false }))
        );
        assert_eq!(
            get().await,
            (StatusCode::OK, json!({ "announcements": false }))
        );

        let (status, _) = json_request(
            state,
            Method::PUT,
            "/api/account/email",
            true,
            Some(json!({ "announcements": "yes" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn unsubscribe_links_work_without_signing_in() {
        let (mut state, _) = state(true);
        state.subscribers = Arc::new(MemorySubscribers::with([Subscriber {
            user_id: "user-123".into(),
            username: "user-123".into(),
            token: TOKEN.into(),
        }]));
        let unsubscribe = |token: &str| {
            let uri = format!("/api/email/unsubscribe/{token}");
            let state = state.clone();
            async move { json_request(state, Method::POST, &uri, false, None).await }
        };

        let wrong = TOKEN.replace('0', "1");
        for token in [wrong.as_str(), "short", &"z".repeat(64)] {
            assert_eq!(
                unsubscribe(token).await,
                (StatusCode::OK, json!({ "unsubscribed": false })),
                "{token}"
            );
        }
        assert_eq!(state.subscribers.list().await.unwrap().len(), 1);
        assert_eq!(
            unsubscribe(TOKEN).await,
            (StatusCode::OK, json!({ "unsubscribed": true }))
        );
        assert_eq!(
            unsubscribe(TOKEN).await,
            (StatusCode::OK, json!({ "unsubscribed": false }))
        );
        assert_eq!(state.subscribers.list().await.unwrap(), []);

        // The route doesn't exist without a token.
        let (status, _) =
            json_request(state, Method::POST, "/api/email/unsubscribe/", false, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn malformed_tokens_never_reach_the_store() {
        let (mut state, _) = state(true);
        state.subscribers = Arc::new(Broken);
        let long = "a".repeat(3000);
        let short = &TOKEN[1..];
        let not_hex = TOKEN.replace('a', "g");
        for token in [long.as_str(), short, not_hex.as_str()] {
            let (status, body) = json_request(
                state.clone(),
                Method::POST,
                &format!("/api/email/unsubscribe/{token}"),
                false,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{token}");
            assert_eq!(body, json!({ "unsubscribed": false }));
        }
    }

    #[tokio::test]
    async fn store_failures_are_server_errors() {
        let (mut state, _) = state(true);
        state.subscribers = Arc::new(Broken);
        for (method, uri, signed_in, body) in [
            (Method::GET, "/api/account/email".to_owned(), true, None),
            (
                Method::PUT,
                "/api/account/email".to_owned(),
                true,
                Some(json!({ "announcements": true })),
            ),
            (
                Method::PUT,
                "/api/account/email".to_owned(),
                true,
                Some(json!({ "announcements": false })),
            ),
            (
                Method::POST,
                format!("/api/email/unsubscribe/{TOKEN}"),
                false,
                None,
            ),
        ] {
            let (status, body) = json_request(state.clone(), method, &uri, signed_in, body).await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{uri}");
            assert_eq!(
                body,
                json!({ "error": "couldn't update your email preferences" })
            );
        }
    }
}
