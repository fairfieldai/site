//! Links between fairfieldct.ai accounts (Cognito users) and Discord accounts.
//!
//! Each link is two items in the site API's `DynamoDB` table, so it can be found
//! from either side:
//!
//! - `PK = USER#<sub>`, `SK = DISCORD`: the Discord ID, username, Discord
//!   refresh token (needed to clear the member's role connection on
//!   disconnect), and when the link was made.
//! - `PK = DISCORD#<discord id>`, `SK = USER`: the Cognito user it belongs to.
//!
//! Saving writes both in one transaction, conditioned so a Discord account
//! can't be linked to two site accounts.

use std::collections::HashMap;
use std::future::Future;
use std::hash::BuildHasher;
use std::pin::Pin;
use std::sync::Mutex;

use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::error::{BuildError, SdkError};
use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError;
use aws_sdk_dynamodb::types::{AttributeValue, Delete, Put, TransactWriteItem};

use crate::Error;

const LINK_SORT_KEY: &str = "DISCORD";
const REVERSE_SORT_KEY: &str = "USER";

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A site account's linked Discord account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// Cognito `sub` of the site account.
    pub user_id: String,
    pub discord_id: String,
    pub discord_username: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub linked_at: i64,
}

#[derive(Debug)]
pub enum SaveError {
    /// The Discord account is linked to a different site account.
    Conflict,
    Store(Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict => f.write_str("Discord account is linked to another user"),
            Self::Store(error) => write!(f, "link store error: {error}"),
        }
    }
}

impl std::error::Error for SaveError {}

/// Where links are kept.
pub trait LinkStore: Send + Sync {
    /// The link for a site account, if any.
    fn get_by_user<'a>(&'a self, user_id: &'a str) -> BoxFuture<'a, Result<Option<Link>, Error>>;

    /// Saves a link, replacing any earlier link for the same site account.
    fn save(&self, link: Link) -> BoxFuture<'_, Result<(), SaveError>>;

    /// Removes a site account's link, returning it.
    fn remove_by_user<'a>(&'a self, user_id: &'a str)
    -> BoxFuture<'a, Result<Option<Link>, Error>>;

    /// Removes the link for a Discord account, returning it.
    fn remove_by_discord<'a>(
        &'a self,
        discord_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<Link>, Error>>;
}

fn user_key(user_id: &str) -> HashMap<String, AttributeValue> {
    HashMap::from([
        ("PK".into(), AttributeValue::S(format!("USER#{user_id}"))),
        ("SK".into(), AttributeValue::S(LINK_SORT_KEY.into())),
    ])
}

fn discord_key(discord_id: &str) -> HashMap<String, AttributeValue> {
    HashMap::from([
        (
            "PK".into(),
            AttributeValue::S(format!("DISCORD#{discord_id}")),
        ),
        ("SK".into(), AttributeValue::S(REVERSE_SORT_KEY.into())),
    ])
}

/// The `USER#` item for a link.
#[must_use]
pub fn link_item(link: &Link) -> HashMap<String, AttributeValue> {
    let mut item = user_key(&link.user_id);
    item.extend([
        ("user_id".into(), AttributeValue::S(link.user_id.clone())),
        (
            "discord_id".into(),
            AttributeValue::S(link.discord_id.clone()),
        ),
        (
            "discord_username".into(),
            AttributeValue::S(link.discord_username.clone()),
        ),
        (
            "refresh_token".into(),
            AttributeValue::S(link.refresh_token.clone()),
        ),
        (
            "linked_at".into(),
            AttributeValue::N(link.linked_at.to_string()),
        ),
    ]);
    item
}

/// The `DISCORD#` item pointing back to the site account.
#[must_use]
pub fn reverse_item(link: &Link) -> HashMap<String, AttributeValue> {
    let mut item = discord_key(&link.discord_id);
    item.insert("user_id".into(), AttributeValue::S(link.user_id.clone()));
    item
}

/// Reads a link from its `USER#` item.
#[must_use]
pub fn link_from_item<S: BuildHasher>(item: &HashMap<String, AttributeValue, S>) -> Option<Link> {
    let text = |name: &str| item.get(name)?.as_s().ok().cloned();
    Some(Link {
        user_id: text("user_id")?,
        discord_id: text("discord_id")?,
        discord_username: text("discord_username")?,
        refresh_token: text("refresh_token")?,
        linked_at: item.get("linked_at")?.as_n().ok()?.parse().ok()?,
    })
}

fn put(table: &str, item: HashMap<String, AttributeValue>) -> Result<Put, BuildError> {
    Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .build()
}

/// Writes for saving `link`, given the site account's current link.
///
/// The reverse item may only be written if it's new or already points at this
/// user, which is what rejects a Discord account linked elsewhere. A previous
/// Discord account's reverse item is deleted.
///
/// # Errors
///
/// If a write can't be built.
pub fn save_writes(
    table: &str,
    link: &Link,
    previous: Option<&Link>,
) -> Result<Vec<TransactWriteItem>, BuildError> {
    let reverse = Put::builder()
        .table_name(table)
        .set_item(Some(reverse_item(link)))
        .condition_expression("attribute_not_exists(PK) OR user_id = :user")
        .expression_attribute_values(":user", AttributeValue::S(link.user_id.clone()))
        .build()?;
    let mut writes = vec![
        TransactWriteItem::builder()
            .put(put(table, link_item(link))?)
            .build(),
        TransactWriteItem::builder().put(reverse).build(),
    ];
    if let Some(previous) = previous.filter(|p| p.discord_id != link.discord_id) {
        let key = discord_key(&previous.discord_id);
        writes.push(
            TransactWriteItem::builder()
                .delete(delete(table, key, "user_id", &link.user_id)?)
                .build(),
        );
    }
    Ok(writes)
}

/// A delete that only applies while `attribute` still equals `value`, so a
/// concurrent re-link isn't undone.
fn delete(
    table: &str,
    key: HashMap<String, AttributeValue>,
    attribute: &str,
    value: &str,
) -> Result<Delete, BuildError> {
    Delete::builder()
        .table_name(table)
        .set_key(Some(key))
        .condition_expression("#attribute = :value")
        .expression_attribute_names("#attribute", attribute)
        .expression_attribute_values(":value", AttributeValue::S(value.into()))
        .build()
}

/// Writes that remove both items of a link.
///
/// # Errors
///
/// If a write can't be built.
pub fn remove_writes(table: &str, link: &Link) -> Result<Vec<TransactWriteItem>, BuildError> {
    let user = delete(
        table,
        user_key(&link.user_id),
        "discord_id",
        &link.discord_id,
    )?;
    let reverse = delete(
        table,
        discord_key(&link.discord_id),
        "user_id",
        &link.user_id,
    )?;
    Ok(vec![
        TransactWriteItem::builder().delete(user).build(),
        TransactWriteItem::builder().delete(reverse).build(),
    ])
}

/// Whether a save was rejected because the Discord account's reverse item
/// belongs to another user. That's the only write in a save whose condition can
/// fail, apart from a concurrent unlink of the previous account.
fn is_conflict(error: &SdkError<TransactWriteItemsError>) -> bool {
    let Some(TransactWriteItemsError::TransactionCanceledException(canceled)) =
        error.as_service_error()
    else {
        return false;
    };
    canceled
        .cancellation_reasons()
        .get(1)
        .and_then(|reason| reason.code())
        == Some("ConditionalCheckFailed")
}

/// Links kept in the site API's `DynamoDB` table.
pub struct DynamoLinks {
    client: Client,
    table: String,
}

impl DynamoLinks {
    #[must_use]
    pub fn new(client: Client, table: String) -> Self {
        Self { client, table }
    }

    async fn get(
        &self,
        key: HashMap<String, AttributeValue>,
    ) -> Result<Option<HashMap<String, AttributeValue>>, Error> {
        Ok(self
            .client
            .get_item()
            .table_name(&self.table)
            .set_key(Some(key))
            .consistent_read(true)
            .send()
            .await?
            .item)
    }

    async fn transact(
        &self,
        writes: Vec<TransactWriteItem>,
    ) -> Result<(), Box<SdkError<TransactWriteItemsError>>> {
        self.client
            .transact_write_items()
            .set_transact_items(Some(writes))
            .send()
            .await
            .map(|_| ())
            .map_err(Box::new)
    }

    async fn remove(&self, link: Option<Link>) -> Result<Option<Link>, Error> {
        let Some(link) = link else {
            return Ok(None);
        };
        self.transact(remove_writes(&self.table, &link)?).await?;
        Ok(Some(link))
    }
}

impl LinkStore for DynamoLinks {
    fn get_by_user<'a>(&'a self, user_id: &'a str) -> BoxFuture<'a, Result<Option<Link>, Error>> {
        Box::pin(async move {
            Ok(self
                .get(user_key(user_id))
                .await?
                .as_ref()
                .and_then(link_from_item))
        })
    }

    fn save(&self, link: Link) -> BoxFuture<'_, Result<(), SaveError>> {
        Box::pin(async move {
            let previous = self
                .get_by_user(&link.user_id)
                .await
                .map_err(SaveError::Store)?;
            let writes = save_writes(&self.table, &link, previous.as_ref())
                .map_err(|error| SaveError::Store(error.into()))?;
            match self.transact(writes).await {
                Ok(()) => Ok(()),
                Err(error) if is_conflict(&error) => Err(SaveError::Conflict),
                Err(error) => Err(SaveError::Store(error)),
            }
        })
    }

    fn remove_by_user<'a>(
        &'a self,
        user_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<Link>, Error>> {
        Box::pin(async move {
            let link = self.get_by_user(user_id).await?;
            self.remove(link).await
        })
    }

    fn remove_by_discord<'a>(
        &'a self,
        discord_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<Link>, Error>> {
        Box::pin(async move {
            let Some(reverse) = self.get(discord_key(discord_id)).await? else {
                return Ok(None);
            };
            let Some(user_id) = reverse.get("user_id").and_then(|v| v.as_s().ok()) else {
                return Ok(None);
            };
            let link = self
                .get_by_user(user_id)
                .await?
                .filter(|link| link.discord_id == discord_id);
            self.remove(link).await
        })
    }
}

/// Links kept in memory, for tests.
#[derive(Default)]
pub struct MemoryLinks {
    links: Mutex<HashMap<String, Link>>,
}

impl MemoryLinks {
    #[must_use]
    pub fn with(links: impl IntoIterator<Item = Link>) -> Self {
        Self {
            links: Mutex::new(links.into_iter().map(|l| (l.user_id.clone(), l)).collect()),
        }
    }

    /// Every stored link.
    ///
    /// # Panics
    ///
    /// If another thread panicked while holding the lock.
    #[must_use]
    pub fn all(&self) -> Vec<Link> {
        self.links.lock().unwrap().values().cloned().collect()
    }
}

impl LinkStore for MemoryLinks {
    fn get_by_user<'a>(&'a self, user_id: &'a str) -> BoxFuture<'a, Result<Option<Link>, Error>> {
        let link = self.links.lock().unwrap().get(user_id).cloned();
        Box::pin(async move { Ok(link) })
    }

    fn save(&self, link: Link) -> BoxFuture<'_, Result<(), SaveError>> {
        let mut links = self.links.lock().unwrap();
        let result = if links
            .values()
            .any(|l| l.discord_id == link.discord_id && l.user_id != link.user_id)
        {
            Err(SaveError::Conflict)
        } else {
            links.insert(link.user_id.clone(), link);
            Ok(())
        };
        Box::pin(async move { result })
    }

    fn remove_by_user<'a>(
        &'a self,
        user_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<Link>, Error>> {
        let link = self.links.lock().unwrap().remove(user_id);
        Box::pin(async move { Ok(link) })
    }

    fn remove_by_discord<'a>(
        &'a self,
        discord_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<Link>, Error>> {
        let mut links = self.links.lock().unwrap();
        let user = links
            .values()
            .find(|l| l.discord_id == discord_id)
            .map(|l| l.user_id.clone());
        let link = user.and_then(|user| links.remove(&user));
        Box::pin(async move { Ok(link) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(user: &str, discord: &str) -> Link {
        Link {
            user_id: user.into(),
            discord_id: discord.into(),
            discord_username: format!("name-{discord}"),
            refresh_token: format!("refresh-{discord}"),
            linked_at: 1_792_000_000,
        }
    }

    fn s(value: &str) -> AttributeValue {
        AttributeValue::S(value.into())
    }

    #[test]
    fn link_items_round_trip() {
        let original = link("u1", "d1");
        let item = link_item(&original);
        assert_eq!(item["PK"], s("USER#u1"));
        assert_eq!(item["SK"], s("DISCORD"));
        assert_eq!(item["linked_at"], AttributeValue::N("1792000000".into()));
        assert_eq!(link_from_item(&item), Some(original));
    }

    #[test]
    fn incomplete_items_are_ignored() {
        let mut item = link_item(&link("u1", "d1"));
        item.remove("refresh_token");
        assert_eq!(link_from_item(&item), None);
        let mut item = link_item(&link("u1", "d1"));
        item.insert("linked_at".into(), s("not a number"));
        assert_eq!(link_from_item(&item), None);
    }

    #[test]
    fn reverse_item_points_at_the_user() {
        let item = reverse_item(&link("u1", "d1"));
        assert_eq!(item["PK"], s("DISCORD#d1"));
        assert_eq!(item["SK"], s("USER"));
        assert_eq!(item["user_id"], s("u1"));
    }

    #[test]
    fn saving_guards_the_discord_account() {
        let writes = save_writes("t", &link("u1", "d1"), None).unwrap();
        assert_eq!(writes.len(), 2);
        let reverse = writes[1].put().unwrap();
        assert_eq!(reverse.item()["PK"], s("DISCORD#d1"));
        assert_eq!(
            reverse.condition_expression(),
            Some("attribute_not_exists(PK) OR user_id = :user")
        );
        assert_eq!(
            reverse.expression_attribute_values().unwrap()[":user"],
            s("u1")
        );
        assert!(writes[0].put().unwrap().condition_expression().is_none());
    }

    #[test]
    fn relinking_another_discord_account_deletes_the_old_reverse_item() {
        let previous = link("u1", "old");
        let writes = save_writes("t", &link("u1", "new"), Some(&previous)).unwrap();
        assert_eq!(writes.len(), 3);
        let delete = writes[2].delete().unwrap();
        assert_eq!(delete.key()["PK"], s("DISCORD#old"));
        assert_eq!(
            delete.expression_attribute_values().unwrap()[":value"],
            s("u1")
        );
        // Relinking the same account doesn't delete anything.
        assert_eq!(
            save_writes("t", &link("u1", "old"), Some(&previous))
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn removing_deletes_both_items_only_if_unchanged() {
        let writes = remove_writes("t", &link("u1", "d1")).unwrap();
        let user = writes[0].delete().unwrap();
        assert_eq!(user.key()["PK"], s("USER#u1"));
        assert_eq!(
            user.expression_attribute_names().unwrap()["#attribute"],
            "discord_id"
        );
        assert_eq!(
            user.expression_attribute_values().unwrap()[":value"],
            s("d1")
        );
        let reverse = writes[1].delete().unwrap();
        assert_eq!(reverse.key()["PK"], s("DISCORD#d1"));
        assert_eq!(
            reverse.expression_attribute_names().unwrap()["#attribute"],
            "user_id"
        );
        assert_eq!(
            reverse.expression_attribute_values().unwrap()[":value"],
            s("u1")
        );
    }

    #[tokio::test]
    async fn memory_store_rejects_a_discord_account_linked_elsewhere() {
        let store = MemoryLinks::with([link("u1", "d1")]);
        assert!(matches!(
            store.save(link("u2", "d1")).await,
            Err(SaveError::Conflict)
        ));
        assert!(store.save(link("u1", "d2")).await.is_ok());
        assert_eq!(
            store
                .remove_by_discord("d2")
                .await
                .unwrap()
                .unwrap()
                .user_id,
            "u1"
        );
        assert_eq!(store.get_by_user("u1").await.unwrap(), None);
        assert_eq!(store.remove_by_user("u1").await.unwrap(), None);
    }
}
