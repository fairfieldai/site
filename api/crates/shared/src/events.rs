//! Meetups shown on the site and members' RSVPs.
//!
//! Organizers create meetups as Discord scheduled events. Discord forgets an
//! event once it ends, so the reminder job copies each one into every site
//! API table, where it stays as a past event. Items:
//!
//! - `PK = EVENTS`, `SK = EVENT#<id>`: the event, its status, and its RSVP
//!   count.
//! - `PK = EVENT#<id>`, `SK = RSVP#<sub>`: a member who's going, with their
//!   Cognito username.
//! - `PK = USER#<sub>`, `SK = RSVP#<id>`: the same RSVP, found from the
//!   member's side.
//!
//! An RSVP and its count change in one transaction.

use std::collections::HashMap;
use std::sync::Mutex;

use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::operation::update_item::UpdateItemError;
use aws_sdk_dynamodb::types::{Delete, Put, TransactWriteItem, Update};
use serde::Serialize;

use crate::Error;
use crate::dynamo::{self, Item, key, n, number, s, text};
use crate::links::BoxFuture;

const EVENTS: &str = "EVENTS";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Scheduled,
    /// Started in Discord and not yet ended.
    Active,
    Ended,
    Canceled,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Active => "active",
            Self::Ended => "ended",
            Self::Canceled => "canceled",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "scheduled" => Some(Self::Scheduled),
            "active" => Some(Self::Active),
            "ended" => Some(Self::Ended),
            "canceled" => Some(Self::Canceled),
            _ => None,
        }
    }

    /// Scheduled or active: Discord still lists it.
    #[must_use]
    pub fn is_open(self) -> bool {
        match self {
            Self::Scheduled | Self::Active => true,
            Self::Ended | Self::Canceled => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Event {
    /// The Discord scheduled event ID.
    pub id: String,
    pub name: String,
    pub description: String,
    /// Unix seconds.
    pub starts_at: i64,
    /// Unix seconds. Discord leaves the end of most events unset, so this may
    /// be an estimate.
    pub ends_at: i64,
    /// Where it happens, or empty if Discord doesn't say.
    pub location: String,
    /// The event on Discord.
    pub url: String,
    pub status: Status,
    pub rsvps: u32,
}

impl Event {
    /// Whether members can still RSVP: open and not over.
    #[must_use]
    pub fn is_upcoming(&self, now: i64) -> bool {
        self.status.is_open() && self.ends_at > now
    }

    /// Whether the details shown on the site differ, ignoring the RSVP count.
    #[must_use]
    pub fn same_details(&self, other: &Self) -> bool {
        Self {
            rsvps: 0,
            ..self.clone()
        } == Self {
            rsvps: 0,
            ..other.clone()
        }
    }
}

/// A member who RSVP'd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attendee {
    /// Cognito `sub`.
    pub user_id: String,
    /// Cognito username, for looking up their email address.
    pub username: String,
}

#[derive(Debug)]
pub enum RsvpError {
    NotFound,
    /// The event is over or canceled.
    Closed,
    Store(Error),
}

impl std::fmt::Display for RsvpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("no such event"),
            Self::Closed => f.write_str("the event is over or canceled"),
            Self::Store(error) => write!(f, "event store error: {error}"),
        }
    }
}

impl std::error::Error for RsvpError {}

impl From<Error> for RsvpError {
    fn from(error: Error) -> Self {
        Self::Store(error)
    }
}

/// Where events and RSVPs are kept.
pub trait EventStore: Send + Sync {
    /// Every event, in no particular order.
    fn list(&self) -> BoxFuture<'_, Result<Vec<Event>, Error>>;

    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<Event>, Error>>;

    /// Saves an event's details and status, keeping its RSVP count.
    fn save<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, Result<(), Error>>;

    /// Marks an open event ended or canceled. Returns false if it wasn't
    /// stored or wasn't open.
    fn close<'a>(&'a self, id: &'a str, status: Status) -> BoxFuture<'a, Result<bool, Error>>;

    /// Records that a member is going to an upcoming event, returning the
    /// event's RSVP count. Repeating an RSVP changes nothing.
    fn rsvp<'a>(
        &'a self,
        event_id: &'a str,
        attendee: &'a Attendee,
        now: i64,
    ) -> BoxFuture<'a, Result<u32, RsvpError>>;

    /// Removes a member's RSVP, returning the event's RSVP count.
    fn cancel_rsvp<'a>(
        &'a self,
        event_id: &'a str,
        user_id: &'a str,
    ) -> BoxFuture<'a, Result<u32, RsvpError>>;

    /// IDs of the events a member RSVP'd to.
    fn rsvps_by_user<'a>(&'a self, user_id: &'a str) -> BoxFuture<'a, Result<Vec<String>, Error>>;

    fn attendees<'a>(&'a self, event_id: &'a str) -> BoxFuture<'a, Result<Vec<Attendee>, Error>>;
}

fn event_key(id: &str) -> Item {
    key(EVENTS, format!("EVENT#{id}"))
}

fn rsvp_key(event_id: &str, user_id: &str) -> Item {
    key(format!("EVENT#{event_id}"), format!("RSVP#{user_id}"))
}

fn user_rsvp_key(user_id: &str, event_id: &str) -> Item {
    key(format!("USER#{user_id}"), format!("RSVP#{event_id}"))
}

/// Reads an event from its item.
#[must_use]
pub fn event_from_item(item: &Item) -> Option<Event> {
    Some(Event {
        id: text(item, "id")?,
        name: text(item, "name")?,
        description: text(item, "description")?,
        starts_at: number(item, "starts_at")?,
        ends_at: number(item, "ends_at")?,
        location: text(item, "location")?,
        url: text(item, "url")?,
        status: Status::parse(&text(item, "status")?)?,
        rsvps: number(item, "rsvps").unwrap_or(0),
    })
}

/// The update that saves an event's details without touching its RSVP count.
///
/// # Errors
///
/// If the update can't be built.
pub fn save_update(
    table: &str,
    event: &Event,
) -> Result<Update, aws_sdk_dynamodb::error::BuildError> {
    Update::builder()
        .table_name(table)
        .set_key(Some(event_key(&event.id)))
        .update_expression(
            "SET id = :id, #name = :name, description = :description, starts_at = :starts_at, \
             ends_at = :ends_at, #location = :location, #url = :url, #status = :status, \
             rsvps = if_not_exists(rsvps, :zero)",
        )
        .expression_attribute_names("#name", "name")
        .expression_attribute_names("#location", "location")
        .expression_attribute_names("#url", "url")
        .expression_attribute_names("#status", "status")
        .expression_attribute_values(":id", s(&event.id))
        .expression_attribute_values(":name", s(&event.name))
        .expression_attribute_values(":description", s(&event.description))
        .expression_attribute_values(":starts_at", n(event.starts_at))
        .expression_attribute_values(":ends_at", n(event.ends_at))
        .expression_attribute_values(":location", s(&event.location))
        .expression_attribute_values(":url", s(&event.url))
        .expression_attribute_values(":status", s(event.status.as_str()))
        .expression_attribute_values(":zero", n(0))
        .build()
}

/// Writes for an RSVP: count it if the event is still upcoming, and add both
/// RSVP items if they're new.
///
/// # Errors
///
/// If a write can't be built.
pub fn rsvp_writes(
    table: &str,
    event_id: &str,
    attendee: &Attendee,
    now: i64,
) -> Result<Vec<TransactWriteItem>, aws_sdk_dynamodb::error::BuildError> {
    let count = Update::builder()
        .table_name(table)
        .set_key(Some(event_key(event_id)))
        .update_expression("ADD rsvps :one")
        .condition_expression("#status IN (:scheduled, :active) AND ends_at > :now")
        .expression_attribute_names("#status", "status")
        .expression_attribute_values(":one", n(1))
        .expression_attribute_values(":scheduled", s(Status::Scheduled.as_str()))
        .expression_attribute_values(":active", s(Status::Active.as_str()))
        .expression_attribute_values(":now", n(now))
        .build()?;
    let mut rsvp = rsvp_key(event_id, &attendee.user_id);
    rsvp.extend([
        ("user_id".into(), s(&attendee.user_id)),
        ("username".into(), s(&attendee.username)),
        ("created_at".into(), n(now)),
    ]);
    let mut by_user = user_rsvp_key(&attendee.user_id, event_id);
    by_user.insert("event_id".into(), s(event_id));
    let new = |item| {
        Put::builder()
            .table_name(table)
            .set_item(Some(item))
            .condition_expression("attribute_not_exists(PK)")
            .build()
    };
    Ok(vec![
        TransactWriteItem::builder().update(count).build(),
        TransactWriteItem::builder().put(new(rsvp)?).build(),
        TransactWriteItem::builder().put(new(by_user)?).build(),
    ])
}

/// Writes that remove an RSVP and uncount it, only if it exists.
///
/// # Errors
///
/// If a write can't be built.
pub fn cancel_rsvp_writes(
    table: &str,
    event_id: &str,
    user_id: &str,
) -> Result<Vec<TransactWriteItem>, aws_sdk_dynamodb::error::BuildError> {
    let count = Update::builder()
        .table_name(table)
        .set_key(Some(event_key(event_id)))
        .update_expression("ADD rsvps :minus_one")
        .condition_expression("attribute_exists(PK)")
        .expression_attribute_values(":minus_one", n(-1))
        .build()?;
    let rsvp = Delete::builder()
        .table_name(table)
        .set_key(Some(rsvp_key(event_id, user_id)))
        .condition_expression("attribute_exists(PK)")
        .build()?;
    let by_user = Delete::builder()
        .table_name(table)
        .set_key(Some(user_rsvp_key(user_id, event_id)))
        .build()?;
    Ok(vec![
        TransactWriteItem::builder().update(count).build(),
        TransactWriteItem::builder().delete(rsvp).build(),
        TransactWriteItem::builder().delete(by_user).build(),
    ])
}

/// Whether an RSVP transaction failed because of the event (missing, over, or
/// canceled) rather than only because the member was already going. `failed`
/// lists which writes of [`rsvp_writes`] failed their conditions, if any did.
#[must_use]
pub fn event_check_failed(failed: Option<&[bool]>) -> bool {
    failed.and_then(<[bool]>::first) == Some(&true)
}

/// Events and RSVPs kept in a site API's `DynamoDB` table.
pub struct DynamoEvents {
    client: Client,
    table: String,
}

impl DynamoEvents {
    #[must_use]
    pub fn new(client: Client, table: String) -> Self {
        Self { client, table }
    }

    async fn count(&self, event_id: &str) -> Result<u32, RsvpError> {
        self.get(event_id)
            .await?
            .map(|event| event.rsvps)
            .ok_or(RsvpError::NotFound)
    }

    /// Runs an RSVP transaction. `None` means a condition failed, with which
    /// writes failed theirs.
    async fn transact(&self, writes: Vec<TransactWriteItem>) -> Result<Option<Vec<bool>>, Error> {
        match self
            .client
            .transact_write_items()
            .set_transact_items(Some(writes))
            .send()
            .await
        {
            Ok(_) => Ok(None),
            Err(error) => match dynamo::failed_conditions(&error) {
                Some(failed) if failed.iter().any(|f| *f) => Ok(Some(failed)),
                _ => Err(error.into()),
            },
        }
    }
}

impl EventStore for DynamoEvents {
    fn list(&self) -> BoxFuture<'_, Result<Vec<Event>, Error>> {
        Box::pin(async move {
            let items = dynamo::query(&self.client, &self.table, EVENTS, "EVENT#").await?;
            Ok(items.iter().filter_map(event_from_item).collect())
        })
    }

    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<Event>, Error>> {
        Box::pin(async move {
            let item = dynamo::get(&self.client, &self.table, event_key(id)).await?;
            Ok(item.as_ref().and_then(event_from_item))
        })
    }

    fn save<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let update = save_update(&self.table, event)?;
            self.client
                .update_item()
                .table_name(update.table_name())
                .set_key(Some(update.key().clone()))
                .update_expression(update.update_expression())
                .set_expression_attribute_names(update.expression_attribute_names().cloned())
                .set_expression_attribute_values(update.expression_attribute_values().cloned())
                .send()
                .await?;
            Ok(())
        })
    }

    fn close<'a>(&'a self, id: &'a str, status: Status) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            let result = self
                .client
                .update_item()
                .table_name(&self.table)
                .set_key(Some(event_key(id)))
                .update_expression("SET #status = :status")
                .condition_expression("#status IN (:scheduled, :active)")
                .expression_attribute_names("#status", "status")
                .expression_attribute_values(":status", s(status.as_str()))
                .expression_attribute_values(":scheduled", s(Status::Scheduled.as_str()))
                .expression_attribute_values(":active", s(Status::Active.as_str()))
                .send()
                .await;
            match result {
                Ok(_) => Ok(true),
                Err(error)
                    if error
                        .as_service_error()
                        .is_some_and(UpdateItemError::is_conditional_check_failed_exception) =>
                {
                    Ok(false)
                }
                Err(error) => Err(error.into()),
            }
        })
    }

    fn rsvp<'a>(
        &'a self,
        event_id: &'a str,
        attendee: &'a Attendee,
        now: i64,
    ) -> BoxFuture<'a, Result<u32, RsvpError>> {
        Box::pin(async move {
            let writes = rsvp_writes(&self.table, event_id, attendee, now).map_err(Error::from)?;
            let failed = self.transact(writes).await?;
            if event_check_failed(failed.as_deref()) {
                // The event is missing, over, or canceled.
                return match self.get(event_id).await? {
                    None => Err(RsvpError::NotFound),
                    Some(_) => Err(RsvpError::Closed),
                };
            }
            self.count(event_id).await
        })
    }

    fn cancel_rsvp<'a>(
        &'a self,
        event_id: &'a str,
        user_id: &'a str,
    ) -> BoxFuture<'a, Result<u32, RsvpError>> {
        Box::pin(async move {
            let writes = cancel_rsvp_writes(&self.table, event_id, user_id).map_err(Error::from)?;
            // Whether nothing changed because the event or the RSVP is
            // missing, the count tells the caller where things stand.
            self.transact(writes).await?;
            self.count(event_id).await
        })
    }

    fn rsvps_by_user<'a>(&'a self, user_id: &'a str) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        Box::pin(async move {
            let pk = format!("USER#{user_id}");
            let items = dynamo::query(&self.client, &self.table, &pk, "RSVP#").await?;
            Ok(items
                .iter()
                .filter_map(|item| text(item, "event_id"))
                .collect())
        })
    }

    fn attendees<'a>(&'a self, event_id: &'a str) -> BoxFuture<'a, Result<Vec<Attendee>, Error>> {
        Box::pin(async move {
            let pk = format!("EVENT#{event_id}");
            let items = dynamo::query(&self.client, &self.table, &pk, "RSVP#").await?;
            Ok(items
                .iter()
                .filter_map(|item| {
                    Some(Attendee {
                        user_id: text(item, "user_id")?,
                        username: text(item, "username")?,
                    })
                })
                .collect())
        })
    }
}

/// Events kept in memory, for tests.
#[derive(Default)]
pub struct MemoryEvents {
    events: Mutex<HashMap<String, Event>>,
    rsvps: Mutex<Vec<(String, Attendee)>>,
}

impl MemoryEvents {
    #[must_use]
    pub fn with(events: impl IntoIterator<Item = Event>) -> Self {
        Self {
            events: Mutex::new(events.into_iter().map(|e| (e.id.clone(), e)).collect()),
            rsvps: Mutex::default(),
        }
    }

    fn lookup(&self, id: &str) -> Option<Event> {
        self.events.lock().unwrap().get(id).cloned()
    }
}

impl EventStore for MemoryEvents {
    fn list(&self) -> BoxFuture<'_, Result<Vec<Event>, Error>> {
        let events = self.events.lock().unwrap().values().cloned().collect();
        Box::pin(async move { Ok(events) })
    }

    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<Event>, Error>> {
        let event = self.lookup(id);
        Box::pin(async move { Ok(event) })
    }

    fn save<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, Result<(), Error>> {
        let mut events = self.events.lock().unwrap();
        let rsvps = events.get(&event.id).map_or(0, |e| e.rsvps);
        events.insert(
            event.id.clone(),
            Event {
                rsvps,
                ..event.clone()
            },
        );
        Box::pin(async move { Ok(()) })
    }

    fn close<'a>(&'a self, id: &'a str, status: Status) -> BoxFuture<'a, Result<bool, Error>> {
        let closed = match self.events.lock().unwrap().get_mut(id) {
            Some(event) if event.status.is_open() => {
                event.status = status;
                true
            }
            Some(_) | None => false,
        };
        Box::pin(async move { Ok(closed) })
    }

    fn rsvp<'a>(
        &'a self,
        event_id: &'a str,
        attendee: &'a Attendee,
        now: i64,
    ) -> BoxFuture<'a, Result<u32, RsvpError>> {
        let result = (|| {
            let mut events = self.events.lock().unwrap();
            let event = events.get_mut(event_id).ok_or(RsvpError::NotFound)?;
            if !event.is_upcoming(now) {
                return Err(RsvpError::Closed);
            }
            let mut rsvps = self.rsvps.lock().unwrap();
            if !rsvps
                .iter()
                .any(|(id, a)| id == event_id && a.user_id == attendee.user_id)
            {
                rsvps.push((event_id.into(), attendee.clone()));
                event.rsvps += 1;
            }
            Ok(event.rsvps)
        })();
        Box::pin(async move { result })
    }

    fn cancel_rsvp<'a>(
        &'a self,
        event_id: &'a str,
        user_id: &'a str,
    ) -> BoxFuture<'a, Result<u32, RsvpError>> {
        let result = (|| {
            let mut events = self.events.lock().unwrap();
            let event = events.get_mut(event_id).ok_or(RsvpError::NotFound)?;
            let mut rsvps = self.rsvps.lock().unwrap();
            let before = rsvps.len();
            rsvps.retain(|(id, a)| !(id == event_id && a.user_id == user_id));
            if rsvps.len() < before {
                event.rsvps -= 1;
            }
            Ok(event.rsvps)
        })();
        Box::pin(async move { result })
    }

    fn rsvps_by_user<'a>(&'a self, user_id: &'a str) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        let ids = self
            .rsvps
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, a)| a.user_id == user_id)
            .map(|(id, _)| id.clone())
            .collect();
        Box::pin(async move { Ok(ids) })
    }

    fn attendees<'a>(&'a self, event_id: &'a str) -> BoxFuture<'a, Result<Vec<Attendee>, Error>> {
        let attendees = self
            .rsvps
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| id == event_id)
            .map(|(_, a)| a.clone())
            .collect();
        Box::pin(async move { Ok(attendees) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> Event {
        Event {
            id: "42".into(),
            name: "AI Night".into(),
            description: "Demos".into(),
            starts_at: 1_000,
            ends_at: 2_000,
            location: "Fairfield Library".into(),
            url: "https://discord.com/events/9/42".into(),
            status: Status::Scheduled,
            rsvps: 3,
        }
    }

    fn attendee(id: &str) -> Attendee {
        Attendee {
            user_id: id.into(),
            username: format!("name-{id}"),
        }
    }

    #[test]
    fn events_round_trip_through_their_save_update() {
        let update = save_update("t", &event()).unwrap();
        assert_eq!(update.key()["PK"], s("EVENTS"));
        assert_eq!(update.key()["SK"], s("EVENT#42"));
        // The update never sets the count directly.
        assert!(
            update
                .update_expression()
                .contains("rsvps = if_not_exists(rsvps, :zero)")
        );
        let values = update.expression_attribute_values().unwrap();
        let mut item = update.key().clone();
        for (attribute, value) in [
            ("id", ":id"),
            ("name", ":name"),
            ("description", ":description"),
            ("starts_at", ":starts_at"),
            ("ends_at", ":ends_at"),
            ("location", ":location"),
            ("url", ":url"),
            ("status", ":status"),
        ] {
            item.insert(attribute.into(), values[value].clone());
        }
        item.insert("rsvps".into(), n(3));
        assert_eq!(event_from_item(&item), Some(event()));

        item.insert("status".into(), s("postponed"));
        assert_eq!(event_from_item(&item), None);
        item.insert("status".into(), s("canceled"));
        item.remove("rsvps");
        assert_eq!(event_from_item(&item).unwrap().rsvps, 0);
        item.remove("name");
        assert_eq!(event_from_item(&item), None);
    }

    #[test]
    fn statuses_round_trip() {
        for status in [
            Status::Scheduled,
            Status::Active,
            Status::Ended,
            Status::Canceled,
        ] {
            assert_eq!(Status::parse(status.as_str()), Some(status));
        }
        assert_eq!(Status::parse("Scheduled"), None);
    }

    #[test]
    fn only_a_failed_event_check_closes_rsvps() {
        assert!(!event_check_failed(None));
        assert!(!event_check_failed(Some(&[])));
        assert!(event_check_failed(Some(&[true, false, false])));
        assert!(event_check_failed(Some(&[true, true, true])));
        // Only the RSVP items existed: already going.
        assert!(!event_check_failed(Some(&[false, true, true])));
    }

    #[test]
    fn rsvp_errors_explain_themselves() {
        assert_eq!(RsvpError::NotFound.to_string(), "no such event");
        assert_eq!(
            RsvpError::Closed.to_string(),
            "the event is over or canceled"
        );
        assert_eq!(
            RsvpError::Store("down".into()).to_string(),
            "event store error: down"
        );
    }

    #[test]
    fn upcoming_means_open_and_not_over() {
        let mut e = event();
        assert!(e.is_upcoming(1_999));
        assert!(!e.is_upcoming(2_000));
        e.status = Status::Active;
        assert!(e.is_upcoming(1_500));
        e.status = Status::Canceled;
        assert!(!e.is_upcoming(0));
        e.status = Status::Ended;
        assert!(!e.is_upcoming(0));
    }

    #[test]
    fn details_ignore_the_count() {
        let mut other = event();
        other.rsvps = 9;
        assert!(event().same_details(&other));
        other.name = "Renamed".into();
        assert!(!event().same_details(&other));
    }

    #[test]
    fn rsvps_count_only_for_upcoming_events_and_new_rsvps() {
        let writes = rsvp_writes("t", "42", &attendee("u1"), 1_500).unwrap();
        let count = writes[0].update().unwrap();
        assert_eq!(count.key()["SK"], s("EVENT#42"));
        assert_eq!(count.update_expression(), "ADD rsvps :one");
        assert_eq!(
            count.condition_expression(),
            Some("#status IN (:scheduled, :active) AND ends_at > :now")
        );
        assert_eq!(
            count.expression_attribute_values().unwrap()[":now"],
            n(1_500)
        );
        let rsvp = writes[1].put().unwrap();
        assert_eq!(rsvp.item()["PK"], s("EVENT#42"));
        assert_eq!(rsvp.item()["SK"], s("RSVP#u1"));
        assert_eq!(rsvp.item()["username"], s("name-u1"));
        assert_eq!(
            rsvp.condition_expression(),
            Some("attribute_not_exists(PK)")
        );
        let by_user = writes[2].put().unwrap();
        assert_eq!(by_user.item()["PK"], s("USER#u1"));
        assert_eq!(by_user.item()["SK"], s("RSVP#42"));
        assert_eq!(by_user.item()["event_id"], s("42"));
    }

    #[test]
    fn canceling_uncounts_only_an_existing_rsvp() {
        let writes = cancel_rsvp_writes("t", "42", "u1").unwrap();
        let count = writes[0].update().unwrap();
        assert_eq!(
            count.expression_attribute_values().unwrap()[":minus_one"],
            n(-1)
        );
        let rsvp = writes[1].delete().unwrap();
        assert_eq!(rsvp.key()["SK"], s("RSVP#u1"));
        assert_eq!(rsvp.condition_expression(), Some("attribute_exists(PK)"));
        assert_eq!(writes[2].delete().unwrap().key()["PK"], s("USER#u1"));
    }

    #[tokio::test]
    async fn memory_store_counts_rsvps_once() {
        let store = MemoryEvents::with([Event {
            rsvps: 0,
            ..event()
        }]);
        assert_eq!(store.rsvp("42", &attendee("u1"), 1_500).await.unwrap(), 1);
        assert_eq!(store.rsvp("42", &attendee("u1"), 1_500).await.unwrap(), 1);
        assert_eq!(store.rsvp("42", &attendee("u2"), 1_500).await.unwrap(), 2);
        assert!(matches!(
            store.rsvp("42", &attendee("u3"), 2_000).await,
            Err(RsvpError::Closed)
        ));
        assert!(matches!(
            store.rsvp("nope", &attendee("u3"), 0).await,
            Err(RsvpError::NotFound)
        ));
        assert_eq!(store.rsvps_by_user("u1").await.unwrap(), ["42"]);
        assert_eq!(store.attendees("42").await.unwrap().len(), 2);
        // Canceling another event's RSVP leaves this one alone.
        let other = MemoryEvents::with([Event {
            id: "43".into(),
            ..event()
        }]);
        assert_eq!(other.cancel_rsvp("43", "u1").await.unwrap(), 3);
        store
            .save(&Event {
                id: "43".into(),
                rsvps: 0,
                ..event()
            })
            .await
            .unwrap();
        store.rsvp("43", &attendee("u1"), 1_500).await.unwrap();
        assert_eq!(store.cancel_rsvp("42", "u1").await.unwrap(), 1);
        assert_eq!(store.cancel_rsvp("42", "u1").await.unwrap(), 1);
        assert_eq!(store.rsvps_by_user("u1").await.unwrap(), ["43"]);
        store.cancel_rsvp("43", "u1").await.unwrap();
        assert_eq!(
            store.rsvps_by_user("u1").await.unwrap(),
            Vec::<String>::new()
        );

        // Saving new details keeps the count; closing works once.
        store
            .save(&Event {
                name: "New".into(),
                rsvps: 0,
                ..event()
            })
            .await
            .unwrap();
        assert_eq!(store.get("42").await.unwrap().unwrap().rsvps, 1);
        assert!(store.close("42", Status::Canceled).await.unwrap());
        assert!(!store.close("42", Status::Ended).await.unwrap());
        assert!(!store.close("nope", Status::Ended).await.unwrap());
    }
}
