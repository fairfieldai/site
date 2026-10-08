//! Discord scheduled events: fetching, timestamps, and meetup messages.
//!
//! Organizers create meetups with Discord's Events feature, which makes them
//! the source of truth for `/meetup` and the reminder job.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use lambda_http::Error;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use shared::ssm::Parameter;
use shared::time::parse_rfc3339;

/// Milliseconds between the Unix epoch and the Discord epoch (2015-01-01).
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;
const SCHEDULED: u8 = 1;
const ACTIVE: u8 = 2;
const EXTERNAL: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ScheduledEvent {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub scheduled_start_time: String,
    /// Required for external events; optional for voice and stage events.
    #[serde(default)]
    pub scheduled_end_time: Option<String>,
    pub status: u8,
    pub entity_type: u8,
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub entity_metadata: Option<EntityMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EntityMetadata {
    #[serde(default)]
    pub location: Option<String>,
}

impl ScheduledEvent {
    /// Start time in Unix seconds.
    #[must_use]
    pub fn starts_at(&self) -> Option<i64> {
        parse_rfc3339(&self.scheduled_start_time)
    }

    /// Creation time in Unix seconds, from the event's snowflake ID.
    #[must_use]
    pub fn created_at(&self) -> Option<i64> {
        snowflake_seconds(&self.id)
    }

    #[must_use]
    pub fn is_scheduled(&self) -> bool {
        self.status == SCHEDULED
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.status == ACTIVE
    }

    fn location(&self) -> Option<String> {
        if self.entity_type == EXTERNAL {
            self.entity_metadata.as_ref()?.location.clone()
        } else {
            self.channel_id.as_ref().map(|id| format!("<#{id}>"))
        }
    }

    /// When, where, and a link to the event, below a heading.
    #[must_use]
    pub fn message(&self, heading: &str, guild_id: &str) -> String {
        let mut lines = vec![format!("**{heading}**")];
        if let Some(start) = self.starts_at() {
            lines.push(format!("<t:{start}:F> (<t:{start}:R>)"));
        }
        if let Some(location) = self.location() {
            lines.push(format!("Where: {location}"));
        }
        lines.push(format!("https://discord.com/events/{guild_id}/{}", self.id));
        lines.join("\n")
    }
}

/// The meetup happening now, or else the next scheduled one.
#[must_use]
pub fn next_meetup(events: &[ScheduledEvent], now: i64) -> Option<&ScheduledEvent> {
    events.iter().find(|event| event.is_active()).or_else(|| {
        events
            .iter()
            .filter(|event| event.is_scheduled())
            .filter_map(|event| event.starts_at().map(|start| (start, event)))
            .filter(|(start, _)| *start >= now)
            .min_by_key(|(start, _)| *start)
            .map(|(_, event)| event)
    })
}

/// Unix seconds encoded in a Discord snowflake ID.
#[must_use]
pub fn snowflake_seconds(id: &str) -> Option<i64> {
    let id: u64 = id.parse().ok()?;
    i64::try_from(((id >> 22) + DISCORD_EPOCH_MS) / 1000).ok()
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;

/// Where the server's scheduled events come from.
pub trait EventSource: Send + Sync {
    fn scheduled_events(&self) -> BoxFuture<'_, Vec<ScheduledEvent>>;
}

/// Reads scheduled events from the Discord API as the bot.
pub struct DiscordEvents {
    http: reqwest::Client,
    guild_id: String,
    token: Parameter,
}

impl DiscordEvents {
    /// # Errors
    ///
    /// If the HTTP client can't be built.
    pub fn new(guild_id: String, token: Parameter) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .user_agent("DiscordBot (https://www.fairfieldct.ai, 1.0)")
            .build()?;
        Ok(Self {
            http,
            guild_id,
            token,
        })
    }
}

impl EventSource for DiscordEvents {
    fn scheduled_events(&self) -> BoxFuture<'_, Vec<ScheduledEvent>> {
        Box::pin(async move {
            let token = self.token.value().await?;
            let url = format!(
                "https://discord.com/api/v10/guilds/{}/scheduled-events",
                self.guild_id
            );
            let response = self
                .http
                .get(url)
                .header("Authorization", format!("Bot {token}"))
                .send()
                .await?
                .error_for_status()?;
            Ok(response.json().await?)
        })
    }
}

/// Where meetup announcements are posted.
pub trait Announcer: Send + Sync {
    fn announce(&self, content: String) -> BoxFuture<'_, ()>;
}

/// Posts to a channel through a webhook, so the bot needs no permissions there.
pub struct Webhook {
    http: reqwest::Client,
    url: OnceCell<String>,
    parameter: Parameter,
}

impl Webhook {
    /// # Errors
    ///
    /// If the HTTP client can't be built.
    pub fn new(parameter: Parameter) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()?;
        Ok(Self {
            http,
            url: OnceCell::new(),
            parameter,
        })
    }
}

impl Announcer for Webhook {
    fn announce(&self, content: String) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let url = self
                .url
                .get_or_try_init(|| async { self.parameter.value().await.cloned() })
                .await?;
            self.http
                .post(url)
                .json(&announcement(&content))
                .send()
                .await?
                .error_for_status()?;
            Ok(())
        })
    }
}

/// Webhook body with mentions disabled, so event text can't ping anyone.
#[must_use]
pub fn announcement(content: &str) -> Value {
    json!({ "content": content, "allowed_mentions": { "parse": [] } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: &str, start: &str, status: u8) -> ScheduledEvent {
        ScheduledEvent {
            id: id.into(),
            name: format!("Meetup {id}"),
            description: None,
            scheduled_start_time: start.into(),
            scheduled_end_time: None,
            status,
            entity_type: EXTERNAL,
            channel_id: None,
            entity_metadata: Some(EntityMetadata {
                location: Some("Fairfield Library".into()),
            }),
        }
    }

    #[test]
    fn reads_creation_time_from_snowflakes() {
        // Example from Discord's documentation: 2016-04-30T11:18:25.796Z.
        assert_eq!(snowflake_seconds("175928847299117063"), Some(1_462_015_105));
        assert_eq!(snowflake_seconds("0"), Some(1_420_070_400));
        assert_eq!(snowflake_seconds("not-a-number"), None);
    }

    #[test]
    fn next_meetup_prefers_an_active_event() {
        let events = [
            event("1", "2026-10-20T23:00:00+00:00", SCHEDULED),
            event("2", "2026-10-01T23:00:00+00:00", ACTIVE),
        ];
        assert_eq!(next_meetup(&events, 1_792_000_000).unwrap().id, "2");
    }

    #[test]
    fn next_meetup_is_the_earliest_upcoming_scheduled_event() {
        let now = parse_rfc3339("2026-10-10T00:00:00Z").unwrap();
        let events = [
            event("past", "2026-10-01T23:00:00+00:00", SCHEDULED),
            event("later", "2026-10-30T23:00:00+00:00", SCHEDULED),
            event("canceled", "2026-10-12T23:00:00+00:00", 4),
            event("soonest", "2026-10-15T23:00:00+00:00", SCHEDULED),
            event("bad-time", "whenever", SCHEDULED),
        ];
        assert_eq!(next_meetup(&events, now).unwrap().id, "soonest");
        assert_eq!(next_meetup(&events[..1], now), None);
        assert_eq!(next_meetup(&[], now), None);
    }

    #[test]
    fn message_has_time_place_and_link() {
        let message = event("42", "2026-10-15T23:00:00+00:00", SCHEDULED).message("Next", "9");
        assert_eq!(
            message,
            "**Next**\n<t:1792105200:F> (<t:1792105200:R>)\nWhere: Fairfield Library\nhttps://discord.com/events/9/42"
        );
    }

    #[test]
    fn channel_events_link_the_channel() {
        let mut voice = event("42", "2026-10-15T23:00:00+00:00", SCHEDULED);
        voice.entity_type = 2;
        voice.channel_id = Some("77".into());
        assert!(voice.message("Next", "9").contains("Where: <#77>"));
    }

    #[test]
    fn announcements_never_mention_anyone() {
        assert_eq!(
            announcement("@everyone hi"),
            json!({ "content": "@everyone hi", "allowed_mentions": { "parse": [] } })
        );
    }
}
