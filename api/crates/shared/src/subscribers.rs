//! Members who get meetup announcements by email.
//!
//! Items in the site API's table:
//!
//! - `PK = SUBSCRIBERS`, `SK = USER#<sub>`: the member's Cognito username and
//!   unsubscribe token.
//! - `PK = UNSUBSCRIBE#<token>`, `SK = USER`: the subscriber the token belongs
//!   to, so an email's unsubscribe link works without signing in.
//!
//! Email addresses aren't stored; the sender looks each one up in Cognito.

use std::sync::Mutex;

use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::error::BuildError;
use aws_sdk_dynamodb::types::{Delete, Put, TransactWriteItem};

use crate::Error;
use crate::dynamo::{self, Item, key, s, text};
use crate::links::BoxFuture;

const SUBSCRIBERS: &str = "SUBSCRIBERS";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscriber {
    /// Cognito `sub`.
    pub user_id: String,
    /// Cognito username, for looking up their email address.
    pub username: String,
    /// Secret in the unsubscribe link of every announcement.
    pub token: String,
}

pub trait Subscribers: Send + Sync {
    fn get<'a>(&'a self, user_id: &'a str) -> BoxFuture<'a, Result<Option<Subscriber>, Error>>;

    /// Adds a subscriber. A member who's already subscribed keeps their token.
    fn subscribe(&self, subscriber: Subscriber) -> BoxFuture<'_, Result<(), Error>>;

    /// Removes a member's subscription, returning whether there was one.
    fn unsubscribe<'a>(&'a self, user_id: &'a str) -> BoxFuture<'a, Result<bool, Error>>;

    /// Removes the subscription an unsubscribe token belongs to, returning
    /// whether there was one.
    fn unsubscribe_token<'a>(&'a self, token: &'a str) -> BoxFuture<'a, Result<bool, Error>>;

    fn list(&self) -> BoxFuture<'_, Result<Vec<Subscriber>, Error>>;
}

fn subscriber_key(user_id: &str) -> Item {
    key(SUBSCRIBERS, format!("USER#{user_id}"))
}

fn token_key(token: &str) -> Item {
    key(format!("UNSUBSCRIBE#{token}"), "USER")
}

#[must_use]
pub fn subscriber_from_item(item: &Item) -> Option<Subscriber> {
    Some(Subscriber {
        user_id: text(item, "user_id")?,
        username: text(item, "username")?,
        token: text(item, "token")?,
    })
}

/// Writes adding a subscriber and their token, unless they're already
/// subscribed.
///
/// # Errors
///
/// If a write can't be built.
pub fn subscribe_writes(
    table: &str,
    subscriber: &Subscriber,
) -> Result<Vec<TransactWriteItem>, BuildError> {
    let mut item = subscriber_key(&subscriber.user_id);
    item.extend([
        ("user_id".into(), s(&subscriber.user_id)),
        ("username".into(), s(&subscriber.username)),
        ("token".into(), s(&subscriber.token)),
    ]);
    let mut reverse = token_key(&subscriber.token);
    reverse.insert("user_id".into(), s(&subscriber.user_id));
    let new = |item| {
        Put::builder()
            .table_name(table)
            .set_item(Some(item))
            .condition_expression("attribute_not_exists(PK)")
            .build()
    };
    Ok(vec![
        TransactWriteItem::builder().put(new(item)?).build(),
        TransactWriteItem::builder().put(new(reverse)?).build(),
    ])
}

/// Writes removing a subscriber and their token.
///
/// # Errors
///
/// If a write can't be built.
pub fn unsubscribe_writes(
    table: &str,
    subscriber: &Subscriber,
) -> Result<Vec<TransactWriteItem>, BuildError> {
    let delete = |key| {
        Delete::builder()
            .table_name(table)
            .set_key(Some(key))
            .build()
    };
    Ok(vec![
        TransactWriteItem::builder()
            .delete(delete(subscriber_key(&subscriber.user_id))?)
            .build(),
        TransactWriteItem::builder()
            .delete(delete(token_key(&subscriber.token))?)
            .build(),
    ])
}

pub struct DynamoSubscribers {
    client: Client,
    table: String,
}

impl DynamoSubscribers {
    #[must_use]
    pub fn new(client: Client, table: String) -> Self {
        Self { client, table }
    }

    async fn transact(&self, writes: Vec<TransactWriteItem>) -> Result<bool, Error> {
        match self
            .client
            .transact_write_items()
            .set_transact_items(Some(writes))
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(error) => match dynamo::failed_conditions(&error) {
                Some(failed) if failed.iter().any(|f| *f) => Ok(false),
                _ => Err(error.into()),
            },
        }
    }
}

impl Subscribers for DynamoSubscribers {
    fn get<'a>(&'a self, user_id: &'a str) -> BoxFuture<'a, Result<Option<Subscriber>, Error>> {
        Box::pin(async move {
            let item = dynamo::get(&self.client, &self.table, subscriber_key(user_id)).await?;
            Ok(item.as_ref().and_then(subscriber_from_item))
        })
    }

    fn subscribe(&self, subscriber: Subscriber) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            // A failed condition means they're already subscribed.
            self.transact(subscribe_writes(&self.table, &subscriber)?)
                .await?;
            Ok(())
        })
    }

    fn unsubscribe<'a>(&'a self, user_id: &'a str) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            let Some(subscriber) = self.get(user_id).await? else {
                return Ok(false);
            };
            self.transact(unsubscribe_writes(&self.table, &subscriber)?)
                .await
        })
    }

    fn unsubscribe_token<'a>(&'a self, token: &'a str) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            let Some(item) = dynamo::get(&self.client, &self.table, token_key(token)).await? else {
                return Ok(false);
            };
            let Some(user_id) = text(&item, "user_id") else {
                return Ok(false);
            };
            match self.get(&user_id).await? {
                Some(subscriber) if subscriber.token == token => {
                    self.transact(unsubscribe_writes(&self.table, &subscriber)?)
                        .await
                }
                Some(_) | None => Ok(false),
            }
        })
    }

    fn list(&self) -> BoxFuture<'_, Result<Vec<Subscriber>, Error>> {
        Box::pin(async move {
            let items = dynamo::query(&self.client, &self.table, SUBSCRIBERS, "USER#").await?;
            Ok(items.iter().filter_map(subscriber_from_item).collect())
        })
    }
}

/// Subscribers kept in memory, for tests.
#[derive(Default)]
pub struct MemorySubscribers {
    subscribers: Mutex<Vec<Subscriber>>,
}

impl MemorySubscribers {
    #[must_use]
    pub fn with(subscribers: impl IntoIterator<Item = Subscriber>) -> Self {
        Self {
            subscribers: Mutex::new(subscribers.into_iter().collect()),
        }
    }

    fn remove(&self, matches: impl Fn(&Subscriber) -> bool) -> bool {
        let mut subscribers = self.subscribers.lock().unwrap();
        let before = subscribers.len();
        subscribers.retain(|s| !matches(s));
        subscribers.len() < before
    }
}

impl Subscribers for MemorySubscribers {
    fn get<'a>(&'a self, user_id: &'a str) -> BoxFuture<'a, Result<Option<Subscriber>, Error>> {
        let found = self
            .subscribers
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.user_id == user_id)
            .cloned();
        Box::pin(async move { Ok(found) })
    }

    fn subscribe(&self, subscriber: Subscriber) -> BoxFuture<'_, Result<(), Error>> {
        let mut subscribers = self.subscribers.lock().unwrap();
        if !subscribers.iter().any(|s| s.user_id == subscriber.user_id) {
            subscribers.push(subscriber);
        }
        Box::pin(async move { Ok(()) })
    }

    fn unsubscribe<'a>(&'a self, user_id: &'a str) -> BoxFuture<'a, Result<bool, Error>> {
        let removed = self.remove(|s| s.user_id == user_id);
        Box::pin(async move { Ok(removed) })
    }

    fn unsubscribe_token<'a>(&'a self, token: &'a str) -> BoxFuture<'a, Result<bool, Error>> {
        let removed = self.remove(|s| s.token == token);
        Box::pin(async move { Ok(removed) })
    }

    fn list(&self) -> BoxFuture<'_, Result<Vec<Subscriber>, Error>> {
        let subscribers = self.subscribers.lock().unwrap().clone();
        Box::pin(async move { Ok(subscribers) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subscriber() -> Subscriber {
        Subscriber {
            user_id: "u1".into(),
            username: "name-u1".into(),
            token: "t0k3n".into(),
        }
    }

    #[test]
    fn subscribing_writes_both_items_only_once() {
        let writes = subscribe_writes("t", &subscriber()).unwrap();
        let item = writes[0].put().unwrap();
        assert_eq!(item.item()["PK"], s("SUBSCRIBERS"));
        assert_eq!(item.item()["SK"], s("USER#u1"));
        assert_eq!(
            item.condition_expression(),
            Some("attribute_not_exists(PK)")
        );
        assert_eq!(subscriber_from_item(item.item()), Some(subscriber()));
        let reverse = writes[1].put().unwrap();
        assert_eq!(reverse.item()["PK"], s("UNSUBSCRIBE#t0k3n"));
        assert_eq!(reverse.item()["user_id"], s("u1"));
        assert_eq!(
            reverse.condition_expression(),
            Some("attribute_not_exists(PK)")
        );
    }

    #[test]
    fn unsubscribing_deletes_both_items() {
        let writes = unsubscribe_writes("t", &subscriber()).unwrap();
        assert_eq!(writes[0].delete().unwrap().key()["SK"], s("USER#u1"));
        assert_eq!(
            writes[1].delete().unwrap().key()["PK"],
            s("UNSUBSCRIBE#t0k3n")
        );
    }

    #[test]
    fn incomplete_items_are_ignored() {
        let mut item = subscribe_writes("t", &subscriber()).unwrap()[0]
            .put()
            .unwrap()
            .item()
            .clone();
        item.remove("token");
        assert_eq!(subscriber_from_item(&item), None);
    }

    #[tokio::test]
    async fn memory_store_finds_and_removes_by_user() {
        let other = Subscriber {
            user_id: "u2".into(),
            token: "t2".into(),
            ..subscriber()
        };
        let store = MemorySubscribers::with([subscriber(), other.clone()]);
        assert_eq!(store.get("u1").await.unwrap(), Some(subscriber()));
        assert_eq!(store.get("u3").await.unwrap(), None);
        assert!(store.unsubscribe("u1").await.unwrap());
        assert_eq!(store.list().await.unwrap(), [other]);
    }

    #[tokio::test]
    async fn memory_store_keeps_the_first_token() {
        let store = MemorySubscribers::default();
        store.subscribe(subscriber()).await.unwrap();
        store
            .subscribe(Subscriber {
                token: "other".into(),
                ..subscriber()
            })
            .await
            .unwrap();
        assert_eq!(store.list().await.unwrap(), [subscriber()]);
        assert!(!store.unsubscribe_token("other").await.unwrap());
        assert!(store.unsubscribe_token("t0k3n").await.unwrap());
        assert!(!store.unsubscribe("u1").await.unwrap());
    }
}
