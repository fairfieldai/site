//! Linking a member's Discord account to their fairfieldct.ai account.
//!
//! The site's `/connect/discord/` page sends the member through Discord's
//! OAuth2 consent screen and posts the returned code to
//! `POST /api/account/discord`, which exchanges it, records the link, and (in
//! the environment that manages it) sets the Linked Roles connection so
//! Discord grants the server's Member role. `DELETE` clears the role
//! connection, revokes Discord's tokens, and removes the link.
//!
//! Prod and dev share one Discord application, and Discord keeps a single role
//! connection per user per application, so only one environment
//! (`manage_role_connection`) touches it; the others only record links.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{FromRef, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use lambda_http::{Error, tracing};
use serde::{Deserialize, Serialize};
use serde_json::json;
use shared::links::{Link, LinkStore, SaveError};
use shared::ssm::Parameter;
use shared::time::now;

use crate::auth::User;

const API: &str = "https://discord.com/api/v10";
const PLATFORM_NAME: &str = "fairfieldct.ai";

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, OAuthError>> + Send + 'a>>;

#[derive(Debug)]
pub enum OAuthError {
    /// Discord rejected the code or refresh token (expired, used, or revoked).
    InvalidGrant,
    /// Discord couldn't be reached or returned an unexpected error.
    Failed(Error),
}

impl From<reqwest::Error> for OAuthError {
    fn from(error: reqwest::Error) -> Self {
        Self::Failed(error.into())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub scope: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiscordUser {
    pub id: String,
    pub username: String,
}

/// The Discord OAuth2 and Linked Roles calls linking needs.
pub trait DiscordOAuth: Send + Sync {
    fn exchange_code<'a>(&'a self, code: &'a str) -> BoxFuture<'a, Tokens>;
    fn refresh<'a>(&'a self, refresh_token: &'a str) -> BoxFuture<'a, Tokens>;
    fn current_user<'a>(&'a self, access_token: &'a str) -> BoxFuture<'a, DiscordUser>;
    /// Sets the member's Linked Roles connection for this application.
    fn set_member<'a>(&'a self, access_token: &'a str, member: bool) -> BoxFuture<'a, ()>;
    fn revoke<'a>(&'a self, refresh_token: &'a str) -> BoxFuture<'a, ()>;
}

/// Discord's OAuth2 API, authenticated as the application.
pub struct DiscordHttp {
    http: reqwest::Client,
    application_id: String,
    client_secret: Parameter,
    redirect_uri: String,
}

impl DiscordHttp {
    /// # Errors
    ///
    /// If the HTTP client can't be built.
    pub fn new(
        application_id: String,
        client_secret: Parameter,
        redirect_uri: String,
    ) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .user_agent("DiscordBot (https://www.fairfieldct.ai, 1.0)")
            .build()?;
        Ok(Self {
            http,
            application_id,
            client_secret,
            redirect_uri,
        })
    }

    async fn token(&self, form: &[(&str, &str)]) -> Result<Tokens, OAuthError> {
        let secret = self
            .client_secret
            .value()
            .await
            .map_err(OAuthError::Failed)?;
        let response = self
            .http
            .post(format!("{API}/oauth2/token"))
            .basic_auth(&self.application_id, Some(secret))
            .form(form)
            .send()
            .await?;
        // Discord answers invalid, expired, or revoked grants with a 400.
        if response.status() == reqwest::StatusCode::BAD_REQUEST {
            return Err(OAuthError::InvalidGrant);
        }
        Ok(response.error_for_status()?.json().await?)
    }
}

impl DiscordOAuth for DiscordHttp {
    fn exchange_code<'a>(&'a self, code: &'a str) -> BoxFuture<'a, Tokens> {
        Box::pin(async move {
            self.token(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &self.redirect_uri),
            ])
            .await
        })
    }

    fn refresh<'a>(&'a self, refresh_token: &'a str) -> BoxFuture<'a, Tokens> {
        Box::pin(async move {
            self.token(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ])
            .await
        })
    }

    fn current_user<'a>(&'a self, access_token: &'a str) -> BoxFuture<'a, DiscordUser> {
        Box::pin(async move {
            let response = self
                .http
                .get(format!("{API}/users/@me"))
                .bearer_auth(access_token)
                .send()
                .await?
                .error_for_status()?;
            Ok(response.json().await?)
        })
    }

    fn set_member<'a>(&'a self, access_token: &'a str, member: bool) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.http
                .put(format!(
                    "{API}/users/@me/applications/{}/role-connection",
                    self.application_id
                ))
                .bearer_auth(access_token)
                .json(&role_connection(member))
                .send()
                .await?
                .error_for_status()?;
            Ok(())
        })
    }

    fn revoke<'a>(&'a self, refresh_token: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let secret = self
                .client_secret
                .value()
                .await
                .map_err(OAuthError::Failed)?;
            self.http
                .post(format!("{API}/oauth2/token/revoke"))
                .basic_auth(&self.application_id, Some(secret))
                .form(&[
                    ("token", refresh_token),
                    ("token_type_hint", "refresh_token"),
                ])
                .send()
                .await?
                .error_for_status()?;
            Ok(())
        })
    }
}

/// The Linked Roles connection body. Metadata values are strings; booleans
/// are `"1"` or `"0"`.
#[must_use]
pub fn role_connection(member: bool) -> serde_json::Value {
    json!({
        "platform_name": PLATFORM_NAME,
        "metadata": { "member": if member { "1" } else { "0" } },
    })
}

/// Account linking for one environment.
pub struct DiscordLinking {
    pub oauth: Arc<dyn DiscordOAuth>,
    pub links: Arc<dyn LinkStore>,
    /// Whether this environment owns the Linked Roles connection (prod).
    pub manage_role_connection: bool,
}

impl DiscordLinking {
    /// OAuth2 scopes the site requests: Linked Roles needs
    /// `role_connections.write`; environments that only record links don't.
    #[must_use]
    pub fn scopes(&self) -> &'static str {
        if self.manage_role_connection {
            "identify role_connections.write"
        } else {
            "identify"
        }
    }
}

/// Linking is off in environments without a Discord application configured.
pub type Linking = Option<Arc<DiscordLinking>>;

pub fn routes<S>() -> Router<S>
where
    Linking: FromRef<S>,
    Arc<crate::auth::Verifier>: FromRef<S>,
    S: Clone + Send + Sync + 'static,
{
    Router::new().route(
        "/api/account/discord",
        get(status).post(connect).delete(disconnect),
    )
}

#[derive(Debug, Serialize)]
struct Status {
    linked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    /// Scopes to request on Discord's consent screen.
    scopes: &'static str,
}

#[derive(Deserialize)]
pub struct ConnectRequest {
    code: String,
}

fn error(status: StatusCode, message: &'static str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn unavailable() -> Response {
    error(
        StatusCode::NOT_FOUND,
        "Discord linking isn't available here",
    )
}

fn store_failed(error: &dyn std::fmt::Display) -> Response {
    tracing::error!(%error, "link store failed");
    self::error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "couldn't update the link",
    )
}

fn discord_failed(error: &OAuthError) -> Response {
    tracing::error!(?error, "Discord request failed");
    self::error(StatusCode::BAD_GATEWAY, "Discord didn't respond; try again")
}

async fn status(State(linking): State<Linking>, user: User) -> Response {
    let Some(linking) = linking else {
        return unavailable();
    };
    match linking.links.get_by_user(&user.sub).await {
        Ok(link) => Json(Status {
            linked: link.is_some(),
            username: link.map(|link| link.discord_username),
            scopes: linking.scopes(),
        })
        .into_response(),
        Err(error) => store_failed(&error),
    }
}

async fn connect(
    State(linking): State<Linking>,
    user: User,
    Json(request): Json<ConnectRequest>,
) -> Response {
    let Some(linking) = linking else {
        return unavailable();
    };
    let tokens = match linking.oauth.exchange_code(&request.code).await {
        Ok(tokens) => tokens,
        Err(OAuthError::InvalidGrant) => {
            return error(
                StatusCode::BAD_REQUEST,
                "the Discord authorization expired; try again",
            );
        }
        Err(error) => return discord_failed(&error),
    };
    if linking.manage_role_connection
        && !tokens
            .scope
            .split(' ')
            .any(|s| s == "role_connections.write")
    {
        return error(
            StatusCode::BAD_REQUEST,
            "Discord didn't grant the role connection permission",
        );
    }
    let discord_user = match linking.oauth.current_user(&tokens.access_token).await {
        Ok(discord_user) => discord_user,
        Err(error) => return discord_failed(&error),
    };
    let previous = match linking.links.get_by_user(&user.sub).await {
        Ok(previous) => previous,
        Err(error) => return store_failed(&error),
    };
    let link = Link {
        user_id: user.sub.clone(),
        discord_id: discord_user.id.clone(),
        discord_username: discord_user.username.clone(),
        // Only the environment that manages the role connection needs to act
        // as the member later; the others don't keep Discord credentials.
        refresh_token: if linking.manage_role_connection {
            tokens.refresh_token.clone()
        } else {
            String::new()
        },
        linked_at: now(),
    };
    match linking.links.save(link).await {
        Ok(()) => {}
        Err(SaveError::Conflict) => {
            return error(
                StatusCode::CONFLICT,
                "that Discord account is connected to a different fairfieldct.ai account",
            );
        }
        Err(SaveError::Store(error)) => return store_failed(&error),
    }
    if linking.manage_role_connection {
        if let Err(error) = linking.oauth.set_member(&tokens.access_token, true).await {
            // Without the role connection the link would promise a role the
            // member doesn't have, so undo it and let them retry.
            if let Err(error) = linking.links.remove_by_user(&user.sub).await {
                tracing::error!(%error, "couldn't undo the link after a failed role connection");
            }
            return discord_failed(&error);
        }
        if let Some(previous) = previous.filter(|p| p.discord_id != discord_user.id) {
            clear_role_connection(&linking, &previous).await;
        }
    }
    Json(Status {
        linked: true,
        username: Some(discord_user.username),
        scopes: linking.scopes(),
    })
    .into_response()
}

/// Best effort: clears the role connection of a Discord account that's no
/// longer linked and revokes its tokens.
async fn clear_role_connection(linking: &DiscordLinking, link: &Link) {
    if link.refresh_token.is_empty() {
        return;
    }
    let result = async {
        let tokens = linking.oauth.refresh(&link.refresh_token).await?;
        linking
            .oauth
            .set_member(&tokens.access_token, false)
            .await?;
        linking.oauth.revoke(&tokens.refresh_token).await
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(
            ?error,
            discord_user = link.discord_id,
            "couldn't clear the previous role connection"
        );
    }
}

async fn disconnect(State(linking): State<Linking>, user: User) -> Response {
    let Some(linking) = linking else {
        return unavailable();
    };
    let link = match linking.links.get_by_user(&user.sub).await {
        Ok(Some(link)) => link,
        Ok(None) => return StatusCode::NO_CONTENT.into_response(),
        Err(error) => return store_failed(&error),
    };
    if linking.manage_role_connection && !link.refresh_token.is_empty() {
        match linking.oauth.refresh(&link.refresh_token).await {
            Ok(tokens) => {
                // Discord rotates refresh tokens, so keep the new one before
                // anything else can fail and leave a dead token behind.
                let rotated = Link {
                    refresh_token: tokens.refresh_token.clone(),
                    ..link.clone()
                };
                if let Err(error) = linking.links.save(rotated).await {
                    return store_failed(&error);
                }
                if let Err(error) = linking.oauth.set_member(&tokens.access_token, false).await {
                    return discord_failed(&error);
                }
                if let Err(error) = linking.oauth.revoke(&tokens.refresh_token).await {
                    tracing::warn!(?error, "couldn't revoke Discord tokens");
                }
            }
            // The member already removed the app in Discord, which cleared
            // the role connection.
            Err(OAuthError::InvalidGrant) => {}
            Err(error) => return discord_failed(&error),
        }
    }
    match linking.links.remove_by_user(&user.sub).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => store_failed(&error),
    }
}
