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

use crate::ssm::Parameter;

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

/// Parses an RFC 3339 timestamp such as `2026-10-15T23:00:00.000000+00:00`
/// into Unix seconds, ignoring fractional seconds.
#[must_use]
pub fn parse_rfc3339(timestamp: &str) -> Option<i64> {
    let (date, rest) = timestamp.split_once('T')?;
    let (time, offset) = if let Some(time) = rest.strip_suffix('Z') {
        (time, 0)
    } else {
        let split = rest.rfind(['+', '-'])?;
        let (time, offset) = rest.split_at(split);
        let sign = if offset.starts_with('-') { -1 } else { 1 };
        let (hours, minutes) = offset[1..].split_once(':')?;
        (
            time,
            sign * (number(hours, 23)? * 3600 + number(minutes, 59)? * 60),
        )
    };
    let mut date = date.split('-');
    let (year, month, day) = (date.next()?, date.next()?, date.next()?);
    let mut time = time.split('.').next()?.split(':');
    let (hour, minute, second) = (time.next()?, time.next()?, time.next()?);
    if date.next().is_some() || time.next().is_some() || year.len() != 4 {
        return None;
    }
    let days = days_from_civil(year.parse().ok()?, number(month, 12)?, number(day, 31)?)?;
    Some(
        days * 86_400 + number(hour, 23)? * 3600 + number(minute, 59)? * 60 + number(second, 60)?
            - offset,
    )
}

fn number(digits: &str, max: i64) -> Option<i64> {
    if digits.len() != 2 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value = digits.parse().ok()?;
    (value <= max).then_some(value)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> Option<i64> {
    if month == 0 || day == 0 {
        return None;
    }
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
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
            status,
            entity_type: EXTERNAL,
            channel_id: None,
            entity_metadata: Some(EntityMetadata {
                location: Some("Fairfield Library".into()),
            }),
        }
    }

    #[test]
    fn parses_rfc3339_variants() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339("2026-10-15T23:00:00+00:00"),
            Some(1_792_105_200)
        );
        assert_eq!(
            parse_rfc3339("2026-10-15T23:00:00.123456+00:00"),
            Some(1_792_105_200)
        );
        assert_eq!(
            parse_rfc3339("2026-10-15T19:00:00-04:00"),
            Some(1_792_105_200)
        );
        assert_eq!(parse_rfc3339("2024-02-29T12:00:00Z"), Some(1_709_208_000));
        assert_eq!(parse_rfc3339("2000-03-01T00:00:00Z"), Some(951_868_800));
    }

    #[test]
    fn rejects_malformed_timestamps() {
        for bad in [
            "",
            "2026-10-15",
            "2026-10-15T23:00",
            "2026-13-01T00:00:00Z",
            "2026-10-00T00:00:00Z",
            "2026-10-15T24:00:00Z",
            "2026-10-15T23:60:00Z",
            "2026-10-15T23:00:00",
            "2026-10-15T23:00:00+0000",
            "26-10-15T23:00:00Z",
            "2026-10-15T2a:00:00Z",
            "2026-10-15-01T23:00:00Z",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
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
