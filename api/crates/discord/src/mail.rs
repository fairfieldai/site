//! Sending email through the mailbox API, to addresses looked up in Cognito.

use std::time::Duration;

use aws_sdk_cognitoidentityprovider::operation::admin_get_user::AdminGetUserError;
use lambda_http::Error;
use serde::Serialize;
use tokio::sync::OnceCell;

use shared::ssm::Parameter;

use crate::events::BoxFuture;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Email {
    pub to: String,
    pub subject: String,
    pub text: String,
    /// Extra headers, such as `List-Unsubscribe`.
    pub headers: Vec<(String, String)>,
    /// Identifies the email across retries, so the mailbox API sends it once.
    #[serde(skip)]
    pub idempotency_key: String,
}

pub trait Mailer: Send + Sync {
    fn send(&self, email: Email) -> BoxFuture<'_, ()>;
}

/// The AgentMail-compatible mailbox API, sending as one inbox.
pub struct MailApi {
    http: reqwest::Client,
    url: String,
    key: OnceCell<String>,
    key_parameter: Parameter,
}

impl MailApi {
    /// Sends from `inbox` (its address is its ID) through the API at
    /// `base_url`, authenticated with the key in `key_parameter`.
    ///
    /// # Errors
    ///
    /// If the HTTP client can't be built.
    pub fn new(base_url: &str, inbox: &str, key_parameter: Parameter) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            http,
            url: format!(
                "{}/v0/inboxes/{inbox}/messages/send",
                base_url.trim_end_matches('/')
            ),
            key: OnceCell::new(),
            key_parameter,
        })
    }
}

/// The mailbox API's send body.
#[must_use]
pub fn send_body(email: &Email) -> serde_json::Value {
    let headers: serde_json::Map<String, serde_json::Value> = email
        .headers
        .iter()
        .map(|(name, value)| (name.clone(), value.clone().into()))
        .collect();
    let mut body = serde_json::json!({
        "to": email.to,
        "subject": email.subject,
        "text": email.text,
    });
    if !headers.is_empty() {
        body["headers"] = headers.into();
    }
    body
}

impl Mailer for MailApi {
    fn send(&self, email: Email) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let key = self
                .key
                .get_or_try_init(|| async { self.key_parameter.value().await.cloned() })
                .await?;
            self.http
                .post(&self.url)
                .bearer_auth(key)
                .header("Idempotency-Key", &email.idempotency_key)
                .json(&send_body(&email))
                .send()
                .await?
                .error_for_status()?;
            Ok(())
        })
    }
}

/// Where members' email addresses come from.
pub trait Directory: Send + Sync {
    /// The member's verified email address, or `None` if the account is gone
    /// or its address isn't verified.
    fn email<'a>(&'a self, username: &'a str) -> BoxFuture<'a, Option<String>>;
}

/// Looks members up in the site's Cognito user pool.
pub struct Cognito {
    client: aws_sdk_cognitoidentityprovider::Client,
    user_pool_id: String,
}

impl Cognito {
    #[must_use]
    pub fn new(client: aws_sdk_cognitoidentityprovider::Client, user_pool_id: String) -> Self {
        Self {
            client,
            user_pool_id,
        }
    }
}

/// The address from a user's attributes, if it's verified.
#[must_use]
pub fn verified_email<'a>(
    attributes: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<String> {
    let mut email = None;
    let mut verified = false;
    for (name, value) in attributes {
        match name {
            "email" => email = Some(value.to_owned()),
            "email_verified" => verified = value == "true",
            _ => {}
        }
    }
    email.filter(|_| verified)
}

impl Directory for Cognito {
    fn email<'a>(&'a self, username: &'a str) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            let result = self
                .client
                .admin_get_user()
                .user_pool_id(&self.user_pool_id)
                .username(username)
                .send()
                .await;
            let user = match result {
                Ok(user) => user,
                Err(error)
                    if error
                        .as_service_error()
                        .is_some_and(AdminGetUserError::is_user_not_found_exception) =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(error.into()),
            };
            if !user.enabled {
                return Ok(None);
            }
            Ok(verified_email(user.user_attributes().iter().map(
                |attribute| (attribute.name(), attribute.value().unwrap_or_default()),
            )))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_verified_addresses_are_used() {
        assert_eq!(
            verified_email([
                ("sub", "u1"),
                ("email", "a@example.com"),
                ("email_verified", "true")
            ]),
            Some("a@example.com".into())
        );
        assert_eq!(
            verified_email([("email", "a@example.com"), ("email_verified", "false")]),
            None
        );
        assert_eq!(verified_email([("email", "a@example.com")]), None);
        assert_eq!(verified_email([("email_verified", "true")]), None);
    }

    #[test]
    fn send_body_has_headers_only_when_set() {
        let mut email = Email {
            to: "a@example.com".into(),
            subject: "Hi".into(),
            text: "Hello".into(),
            headers: vec![],
            idempotency_key: "k".into(),
        };
        assert_eq!(
            send_body(&email),
            serde_json::json!({ "to": "a@example.com", "subject": "Hi", "text": "Hello" })
        );
        email
            .headers
            .push(("List-Unsubscribe".into(), "<https://x>".into()));
        assert_eq!(
            send_body(&email)["headers"],
            serde_json::json!({ "List-Unsubscribe": "<https://x>" })
        );
    }
}
