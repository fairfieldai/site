//! `DynamoDB` helpers shared by the stores in this crate.

use std::collections::HashMap;

use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::error::SdkError;
use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError;
use aws_sdk_dynamodb::types::AttributeValue;

use crate::Error;

pub type Item = HashMap<String, AttributeValue>;

pub fn s(value: impl Into<String>) -> AttributeValue {
    AttributeValue::S(value.into())
}

pub fn n(value: impl std::fmt::Display) -> AttributeValue {
    AttributeValue::N(value.to_string())
}

pub fn key(pk: impl Into<String>, sk: impl Into<String>) -> Item {
    HashMap::from([("PK".into(), s(pk)), ("SK".into(), s(sk))])
}

pub fn text(item: &Item, name: &str) -> Option<String> {
    item.get(name)?.as_s().ok().cloned()
}

pub fn number<T: std::str::FromStr>(item: &Item, name: &str) -> Option<T> {
    item.get(name)?.as_n().ok()?.parse().ok()
}

pub async fn get(client: &Client, table: &str, key: Item) -> Result<Option<Item>, Error> {
    Ok(client
        .get_item()
        .table_name(table)
        .set_key(Some(key))
        .consistent_read(true)
        .send()
        .await?
        .item)
}

/// Every item in partition `pk` whose sort key starts with `prefix`.
pub async fn query(
    client: &Client,
    table: &str,
    pk: &str,
    prefix: &str,
) -> Result<Vec<Item>, Error> {
    let mut items = Vec::new();
    let mut start = None;
    loop {
        let page = client
            .query()
            .table_name(table)
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)")
            .expression_attribute_values(":pk", s(pk))
            .expression_attribute_values(":prefix", s(prefix))
            .consistent_read(true)
            .set_exclusive_start_key(start)
            .send()
            .await?;
        items.extend(page.items.unwrap_or_default());
        start = page.last_evaluated_key;
        if start.is_none() {
            return Ok(items);
        }
    }
}

/// Which writes of a canceled transaction failed their condition, by index.
pub fn failed_conditions(error: &SdkError<TransactWriteItemsError>) -> Option<Vec<bool>> {
    let Some(TransactWriteItemsError::TransactionCanceledException(canceled)) =
        error.as_service_error()
    else {
        return None;
    };
    Some(
        canceled
            .cancellation_reasons()
            .iter()
            .map(|reason| reason.code() == Some("ConditionalCheckFailed"))
            .collect(),
    )
}
