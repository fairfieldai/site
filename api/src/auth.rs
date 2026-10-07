//! Cognito access token verification.
//!
//! Handlers take a [`User`] argument to require a valid access token from the
//! site's app client. Signing keys come from the user pool's JWKS, cached and
//! refetched when a token names an unknown key.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Minimum time between JWKS fetches triggered by unknown key IDs, so forged
/// tokens can't drive a fetch per request.
const MIN_REFETCH_INTERVAL: Duration = Duration::from_secs(300);

/// Where signing keys come from. Production fetches the user pool's JWKS.
pub trait KeySource: Send + Sync {
    fn fetch(&self) -> Pin<Box<dyn Future<Output = Result<JwkSet, AuthError>> + Send + '_>>;
}

pub struct CognitoKeys {
    client: reqwest::Client,
    url: String,
}

impl CognitoKeys {
    #[must_use]
    pub fn new(client: reqwest::Client, issuer: &str) -> Self {
        Self {
            client,
            url: format!("{issuer}/.well-known/jwks.json"),
        }
    }
}

impl KeySource for CognitoKeys {
    fn fetch(&self) -> Pin<Box<dyn Future<Output = Result<JwkSet, AuthError>> + Send + '_>> {
        Box::pin(async move {
            let response = self
                .client
                .get(&self.url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|_| AuthError::Unavailable)?;
            response.json().await.map_err(|_| AuthError::Unavailable)
        })
    }
}

#[derive(Default)]
struct KeyCache {
    by_kid: HashMap<String, DecodingKey>,
    fetched_at: Option<Instant>,
}

pub struct Verifier {
    issuer: String,
    client_id: String,
    source: Box<dyn KeySource>,
    cache: RwLock<KeyCache>,
}

/// The signed-in user behind a verified access token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct User {
    pub sub: String,
    pub username: String,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    username: String,
    token_use: String,
    client_id: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AuthError {
    /// The bearer token is missing or failed verification.
    Invalid,
    /// Signing keys could not be fetched, so validity is unknown.
    Unavailable,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct Body {
            error: &'static str,
        }

        match self {
            Self::Invalid => {
                let mut response = (
                    StatusCode::UNAUTHORIZED,
                    Json(Body {
                        error: "unauthorized",
                    }),
                )
                    .into_response();
                response
                    .headers_mut()
                    .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
                response
            }
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(Body {
                    error: "authentication unavailable",
                }),
            )
                .into_response(),
        }
    }
}

impl Verifier {
    #[must_use]
    pub fn new(issuer: String, client_id: String, source: Box<dyn KeySource>) -> Self {
        Self {
            issuer,
            client_id,
            source,
            cache: RwLock::default(),
        }
    }

    /// Verifies a Cognito access token issued to this verifier's app client.
    ///
    /// # Errors
    ///
    /// [`AuthError::Invalid`] if the token is malformed, expired, not RS256,
    /// signed by an unknown key, or issued for another pool, client, or token
    /// use;
    /// [`AuthError::Unavailable`] if signing keys can't be fetched.
    pub async fn verify(&self, token: &str) -> Result<User, AuthError> {
        let kid = decode_header(token)
            .map_err(|_| AuthError::Invalid)?
            .kid
            .ok_or(AuthError::Invalid)?;
        let key = self.key(&kid).await?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.issuer]);
        // Cognito access tokens carry client_id instead of aud.
        validation.validate_aud = false;
        validation.set_required_spec_claims(&["exp", "iss", "sub"]);

        let claims = decode::<Claims>(token, &key, &validation)
            .map_err(|_| AuthError::Invalid)?
            .claims;
        if claims.token_use != "access" || claims.client_id != self.client_id {
            return Err(AuthError::Invalid);
        }
        Ok(User {
            sub: claims.sub,
            username: claims.username,
        })
    }

    async fn key(&self, kid: &str) -> Result<DecodingKey, AuthError> {
        if let Some(key) = self.cache.read().await.by_kid.get(kid) {
            return Ok(key.clone());
        }

        let mut cache = self.cache.write().await;
        // Another request may have refreshed while this one waited.
        if let Some(key) = cache.by_kid.get(kid) {
            return Ok(key.clone());
        }
        if cache
            .fetched_at
            .is_some_and(|at| at.elapsed() < MIN_REFETCH_INTERVAL)
        {
            return Err(AuthError::Invalid);
        }

        let set = self.source.fetch().await?;
        cache.by_kid = set
            .keys
            .iter()
            .filter_map(|jwk| {
                let kid = jwk.common.key_id.clone()?;
                let key = DecodingKey::from_jwk(jwk).ok()?;
                Some((kid, key))
            })
            .collect();
        cache.fetched_at = Some(Instant::now());
        cache.by_kid.get(kid).cloned().ok_or(AuthError::Invalid)
    }
}

impl<S> FromRequestParts<S> for User
where
    Arc<Verifier>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or(AuthError::Invalid)?;
        Arc::<Verifier>::from_ref(state).verify(token).await
    }
}
