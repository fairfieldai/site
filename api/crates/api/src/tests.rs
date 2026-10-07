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
            discord: None,
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
