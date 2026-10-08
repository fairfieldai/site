//! Keeping each site's copy of Discord's meetups current, and emailing
//! members about them.
//!
//! Every run copies new and changed events into each site API table and closes
//! the ones Discord no longer lists: canceled if they hadn't started, ended
//! otherwise. Where a site emails its members (prod), it also sends:
//!
//! - new-meetup announcements to subscribers, in the run that posts the
//!   Discord announcement;
//! - day-before reminders to subscribers and members who RSVP'd;
//! - cancellation notices to members who RSVP'd, before the event is marked
//!   canceled, so a failed send is retried by the next run.
//!
//! Each email carries an idempotency key, so retried runs don't send twice.
//! Failures here are logged and don't fail the run, because a retried run
//! would repeat its Discord posts.

use std::collections::HashMap;
use std::sync::Arc;

use lambda_http::{Error, tracing};
use shared::events::{Event, EventStore, Status};
use shared::subscribers::Subscribers;
use shared::time::{eastern, parse_rfc3339};

use crate::events::ScheduledEvent;
use crate::mail::{Directory, Email, Mailer};
use crate::reminders::{Reminder, due};

/// How long an event lasts when Discord doesn't give an end time.
const DEFAULT_LENGTH: i64 = 2 * 3600;
const DISCORD_SCHEDULED: u8 = 1;
const DISCORD_ACTIVE: u8 = 2;
const DISCORD_COMPLETED: u8 = 3;
const DISCORD_CANCELED: u8 = 4;
const EXTERNAL: u8 = 3;

/// One environment's site.
pub struct Site {
    pub events: Arc<dyn EventStore>,
    /// Set where the site emails its members.
    pub mail: Option<Mail>,
}

pub struct Mail {
    pub subscribers: Arc<dyn Subscribers>,
    pub directory: Arc<dyn Directory>,
    pub mailer: Arc<dyn Mailer>,
    /// The site's origin, e.g. `https://www.fairfieldct.ai`.
    pub site_url: String,
}

/// The site's version of a Discord event, or `None` if it has no valid start
/// time or an unknown status.
#[must_use]
pub fn site_event(event: &ScheduledEvent, guild_id: &str) -> Option<Event> {
    let starts_at = event.starts_at()?;
    let ends_at = event
        .scheduled_end_time
        .as_deref()
        .and_then(parse_rfc3339)
        .filter(|end| *end > starts_at)
        .unwrap_or(starts_at + DEFAULT_LENGTH);
    let status = match event.status {
        DISCORD_SCHEDULED => Status::Scheduled,
        DISCORD_ACTIVE => Status::Active,
        DISCORD_COMPLETED => Status::Ended,
        DISCORD_CANCELED => Status::Canceled,
        _ => return None,
    };
    let location = if event.entity_type == EXTERNAL {
        event
            .entity_metadata
            .as_ref()
            .and_then(|metadata| metadata.location.clone())
            .unwrap_or_default()
    } else {
        "Online, in our Discord server".into()
    };
    Some(Event {
        id: event.id.clone(),
        name: event.name.clone(),
        description: event.description.clone().unwrap_or_default(),
        starts_at,
        ends_at,
        location,
        url: format!("https://discord.com/events/{guild_id}/{}", event.id),
        status,
        rsvps: 0,
    })
}

/// What a sync writes to one site's table.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// New or changed open events.
    pub save: Vec<Event>,
    /// Stored open events to close, with their new status.
    pub close: Vec<(Event, Status)>,
}

/// Brings `stored` in line with Discord's current events.
#[must_use]
pub fn changes(stored: &[Event], discord: &[ScheduledEvent], guild_id: &str, now: i64) -> Changes {
    let stored_by_id: HashMap<&str, &Event> = stored.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut listed = Vec::new();
    let mut changes = Changes::default();
    for event in discord
        .iter()
        .filter_map(|event| site_event(event, guild_id))
    {
        listed.push(event.id.clone());
        let previous = stored_by_id.get(event.id.as_str());
        if event.status.is_open() {
            if previous.is_none_or(|previous| !previous.same_details(&event)) {
                changes.save.push(event);
            }
        } else if let Some(previous) = previous.filter(|p| p.status.is_open()) {
            changes.close.push(((*previous).clone(), event.status));
        }
    }
    for event in stored {
        if event.status.is_open() && !listed.contains(&event.id) {
            let status = if event.starts_at > now {
                Status::Canceled
            } else {
                Status::Ended
            };
            changes.close.push((event.clone(), status));
        }
    }
    changes
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    New,
    Tomorrow,
    Canceled,
}

impl Notice {
    fn key(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Tomorrow => "tomorrow",
            Self::Canceled => "canceled",
        }
    }
}

/// Someone to email about an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    pub user_id: String,
    pub username: String,
    /// Set for announcement subscribers, for the unsubscribe link.
    pub unsubscribe_token: Option<String>,
    /// Whether they RSVP'd.
    pub going: bool,
}

/// The email telling `recipient` about `event`.
#[must_use]
pub fn email(
    event: &Event,
    notice: Notice,
    recipient: &Recipient,
    to: &str,
    site_url: &str,
) -> Email {
    let page = format!("{site_url}/events/#event-{}", event.id);
    let when = eastern(event.starts_at);
    let (subject, opening) = match notice {
        Notice::New => (
            format!("New meetup: {}", event.name),
            "A new fairfieldct.ai meetup is on the calendar.".to_owned(),
        ),
        Notice::Tomorrow => (
            format!("Tomorrow: {}", event.name),
            if recipient.going {
                "See you tomorrow! You RSVP'd to this meetup.".to_owned()
            } else {
                "This fairfieldct.ai meetup is tomorrow.".to_owned()
            },
        ),
        Notice::Canceled => (
            format!("Canceled: {}", event.name),
            format!("Sorry, this meetup on {when} has been canceled."),
        ),
    };

    let mut lines = vec![opening, String::new(), event.name.clone(), when];
    if !event.location.is_empty() {
        lines.push(event.location.clone());
    }
    if notice != Notice::Canceled && !event.description.is_empty() {
        lines.push(String::new());
        lines.push(event.description.trim().to_owned());
    }
    lines.push(String::new());
    lines.push(match (notice, recipient.going) {
        (Notice::Canceled, _) => format!("See what else is coming up: {site_url}/events/"),
        (Notice::New | Notice::Tomorrow, true) => {
            format!("Can't make it? Update your RSVP: {page}")
        }
        (Notice::New | Notice::Tomorrow, false) => format!("Details and RSVP: {page}"),
    });

    lines.push(String::new());
    lines.push("--".to_owned());
    let mut headers = Vec::new();
    match (&recipient.unsubscribe_token, notice) {
        (Some(token), Notice::New | Notice::Tomorrow) => {
            lines.push(
                "You're getting this because you turned on meetup emails for your fairfieldct.ai account."
                    .to_owned(),
            );
            lines.push(format!("Unsubscribe: {site_url}/unsubscribe/#{token}"));
            headers.push((
                "List-Unsubscribe".to_owned(),
                format!("<{site_url}/api/email/unsubscribe/{token}>"),
            ));
            headers.push((
                "List-Unsubscribe-Post".to_owned(),
                "List-Unsubscribe=One-Click".to_owned(),
            ));
        }
        (Some(_) | None, _) => {
            lines.push("You're getting this because you RSVP'd on fairfieldct.ai.".to_owned());
        }
    }
    lines.push(format!("Email settings: {site_url}/account/"));
    let mut text = lines.join("\n");
    text.push('\n');

    Email {
        to: to.to_owned(),
        subject,
        text,
        headers,
        idempotency_key: format!(
            "{}-{}-{}-{}",
            notice.key(),
            event.id,
            event.starts_at,
            recipient.user_id
        ),
    }
}

impl Mail {
    async fn recipients(
        &self,
        site: &dyn EventStore,
        event: &Event,
        notice: Notice,
    ) -> Result<Vec<Recipient>, Error> {
        let mut recipients: Vec<Recipient> = Vec::new();
        if notice != Notice::Canceled {
            for subscriber in self.subscribers.list().await? {
                recipients.push(Recipient {
                    user_id: subscriber.user_id,
                    username: subscriber.username,
                    unsubscribe_token: Some(subscriber.token),
                    going: false,
                });
            }
        }
        if notice != Notice::New {
            for attendee in site.attendees(&event.id).await? {
                if let Some(existing) = recipients
                    .iter_mut()
                    .find(|r| r.user_id == attendee.user_id)
                {
                    existing.going = true;
                } else {
                    recipients.push(Recipient {
                        user_id: attendee.user_id,
                        username: attendee.username,
                        unsubscribe_token: None,
                        going: true,
                    });
                }
            }
        }
        Ok(recipients)
    }

    /// Emails everyone who should hear about `event`, returning how many
    /// emails were sent.
    ///
    /// # Errors
    ///
    /// If the recipients can't be listed, or any email fails. Every recipient
    /// is tried before returning.
    pub async fn notify(
        &self,
        site: &dyn EventStore,
        event: &Event,
        notice: Notice,
    ) -> Result<usize, Error> {
        let mut sent = 0;
        let mut failures = 0;
        for recipient in self.recipients(site, event, notice).await? {
            let address = match self.directory.email(&recipient.username).await {
                Ok(Some(address)) => address,
                Ok(None) => {
                    tracing::info!(
                        user = recipient.user_id,
                        "no verified email address; skipping"
                    );
                    continue;
                }
                Err(error) => {
                    tracing::error!(%error, user = recipient.user_id, "couldn't look up email address");
                    failures += 1;
                    continue;
                }
            };
            let message = email(event, notice, &recipient, &address, &self.site_url);
            match self.mailer.send(message).await {
                Ok(()) => sent += 1,
                Err(error) => {
                    tracing::error!(%error, user = recipient.user_id, event = event.id, ?notice, "couldn't send email");
                    failures += 1;
                }
            }
        }
        if failures > 0 {
            return Err(format!("{failures} emails about event {} failed", event.id).into());
        }
        Ok(sent)
    }
}

/// What a run did, for logs and tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub saved: usize,
    pub closed: usize,
    pub emailed: usize,
    pub failures: usize,
}

async fn sync(
    site: &Site,
    discord: &[ScheduledEvent],
    guild_id: &str,
    now: i64,
    summary: &mut Summary,
) -> Result<(), Error> {
    let stored = site.events.list().await?;
    let changes = changes(&stored, discord, guild_id, now);
    for event in &changes.save {
        site.events.save(event).await?;
        summary.saved += 1;
    }
    for (event, status) in &changes.close {
        if let (Status::Canceled, Some(mail)) = (status, &site.mail) {
            match mail
                .notify(site.events.as_ref(), event, Notice::Canceled)
                .await
            {
                Ok(sent) => summary.emailed += sent,
                // Leave it open so the next run tries again.
                Err(error) => {
                    tracing::error!(%error, event = event.id, "cancellation emails failed");
                    summary.failures += 1;
                    continue;
                }
            }
        }
        if site.events.close(&event.id, *status).await? {
            summary.closed += 1;
        }
    }
    Ok(())
}

/// Syncs every site and sends the emails due in the run at `run_at`,
/// covering `interval` seconds (see [`due`]).
pub async fn run(
    sites: &[Site],
    discord: &[ScheduledEvent],
    guild_id: &str,
    run_at: i64,
    interval: i64,
) -> Summary {
    let mut summary = Summary::default();
    for site in sites {
        if let Err(error) = sync(site, discord, guild_id, run_at, &mut summary).await {
            tracing::error!(%error, "event sync failed");
            summary.failures += 1;
        }
    }
    for site in sites {
        let Some(mail) = &site.mail else {
            continue;
        };
        for (reminder, event) in due(discord, run_at, interval) {
            let notice = match reminder {
                Reminder::New => Notice::New,
                Reminder::Day => Notice::Tomorrow,
                Reminder::Week | Reminder::Soon => continue,
            };
            let Some(event) = site_event(event, guild_id) else {
                continue;
            };
            match mail.notify(site.events.as_ref(), &event, notice).await {
                Ok(sent) => summary.emailed += sent,
                Err(error) => {
                    tracing::error!(%error, event = event.id, ?notice, "meetup emails failed");
                    summary.failures += 1;
                }
            }
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use shared::events::{Attendee, MemoryEvents};
    use shared::subscribers::{MemorySubscribers, Subscriber};

    use super::*;
    use crate::events::{BoxFuture, EntityMetadata};

    const GUILD: &str = "9";
    const SITE: &str = "https://www.fairfieldct.ai";
    const INTERVAL: i64 = 15 * 60;
    const DISCORD_EPOCH: i64 = 1_420_070_400;

    fn at(timestamp: &str) -> i64 {
        parse_rfc3339(timestamp).unwrap()
    }

    /// A Discord event created at `created` (Unix seconds).
    fn discord(created: i64, start: &str, status: u8) -> ScheduledEvent {
        let id = u64::try_from((created - DISCORD_EPOCH) * 1000).unwrap() << 22;
        ScheduledEvent {
            id: id.to_string(),
            name: "AI Night".into(),
            description: Some("Demos and pizza.".into()),
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

    fn stored(id: &str, starts_at: i64, status: Status) -> Event {
        Event {
            id: id.into(),
            name: "AI Night".into(),
            description: "Demos and pizza.".into(),
            starts_at,
            ends_at: starts_at + DEFAULT_LENGTH,
            location: "Fairfield Library".into(),
            url: format!("https://discord.com/events/{GUILD}/{id}"),
            status,
            rsvps: 0,
        }
    }

    fn subscriber(id: &str) -> Subscriber {
        Subscriber {
            user_id: id.into(),
            username: format!("name-{id}"),
            token: format!("token-{id}"),
        }
    }

    fn attendee(id: &str) -> Attendee {
        Attendee {
            user_id: id.into(),
            username: format!("name-{id}"),
        }
    }

    fn recipient(token: Option<&str>, going: bool) -> Recipient {
        Recipient {
            user_id: "u1".into(),
            username: "name-u1".into(),
            unsubscribe_token: token.map(Into::into),
            going,
        }
    }

    #[test]
    fn converts_discord_events() {
        let mut event = discord(at("2026-09-01T00:00:00Z"), "2026-10-15T23:00:00Z", 1);
        let site = site_event(&event, GUILD).unwrap();
        assert_eq!(site.starts_at, at("2026-10-15T23:00:00Z"));
        assert_eq!(site.ends_at, at("2026-10-16T01:00:00Z"));
        assert_eq!(site.location, "Fairfield Library");
        assert_eq!(site.description, "Demos and pizza.");
        assert_eq!(
            site.url,
            format!("https://discord.com/events/9/{}", event.id)
        );
        assert_eq!(site.status, Status::Scheduled);

        event.scheduled_end_time = Some("2026-10-16T01:30:00Z".into());
        assert_eq!(
            site_event(&event, GUILD).unwrap().ends_at,
            at("2026-10-16T01:30:00Z")
        );
        // An end at or before the start is ignored.
        for end in ["2026-10-15T22:00:00Z", "2026-10-15T23:00:00Z"] {
            event.scheduled_end_time = Some(end.into());
            assert_eq!(
                site_event(&event, GUILD).unwrap().ends_at,
                site.ends_at,
                "{end}"
            );
        }

        for (code, status) in [
            (2, Status::Active),
            (3, Status::Ended),
            (4, Status::Canceled),
        ] {
            event.status = code;
            assert_eq!(site_event(&event, GUILD).unwrap().status, status);
        }
        event.status = 9;
        assert_eq!(site_event(&event, GUILD), None);

        let mut voice = discord(DISCORD_EPOCH, "2026-10-15T23:00:00Z", 1);
        voice.entity_type = 2;
        voice.channel_id = Some("77".into());
        voice.description = None;
        let voice = site_event(&voice, GUILD).unwrap();
        assert_eq!(voice.location, "Online, in our Discord server");
        assert_eq!(voice.description, "");

        let mut nowhere = discord(DISCORD_EPOCH, "2026-10-15T23:00:00Z", 1);
        nowhere.entity_metadata = None;
        assert_eq!(site_event(&nowhere, GUILD).unwrap().location, "");
        assert_eq!(site_event(&discord(DISCORD_EPOCH, "soon", 1), GUILD), None);
    }

    #[test]
    fn saves_new_and_changed_events_only() {
        let now = at("2026-10-01T00:00:00Z");
        let new = discord(now - 60, "2026-10-15T23:00:00Z", 1);
        let same = discord(now - 120, "2026-10-20T23:00:00Z", 1);
        let mut renamed = discord(now - 180, "2026-10-25T23:00:00Z", 1);
        let mut started = discord(now - 240, "2026-09-30T23:30:00Z", 1);
        let stored_events = [
            Event {
                rsvps: 4,
                ..site_event(&same, GUILD).unwrap()
            },
            site_event(&renamed, GUILD).unwrap(),
            site_event(&started, GUILD).unwrap(),
        ];
        renamed.name = "AI Night (new date)".into();
        started.status = 2;
        let changes = changes(
            &stored_events,
            &[new.clone(), same, renamed.clone(), started.clone()],
            GUILD,
            now,
        );
        let saved: Vec<&str> = changes.save.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            saved,
            [new.id.as_str(), renamed.id.as_str(), started.id.as_str()]
        );
        assert_eq!(changes.close, []);
    }

    #[test]
    fn closes_events_discord_dropped_or_finished() {
        let now = at("2026-10-15T00:00:00Z");
        let future = stored("1", now + 1, Status::Scheduled);
        // Starting right now counts as having started.
        let past = stored("2", now, Status::Active);
        let already = stored("3", now + 3600, Status::Canceled);
        let mut completed = discord(DISCORD_EPOCH, "2026-10-14T23:00:00Z", 3);
        completed.id = "4".into();
        let mut canceled = discord(DISCORD_EPOCH, "2026-10-16T23:00:00Z", 4);
        canceled.id = "5".into();
        let mut unknown_canceled = discord(DISCORD_EPOCH, "2026-10-16T23:00:00Z", 4);
        unknown_canceled.id = "6".into();
        let stored_events = [
            future.clone(),
            past.clone(),
            already,
            stored("4", now - 3600, Status::Active),
            stored("5", now + DEFAULT_LENGTH, Status::Scheduled),
        ];
        let changes = changes(
            &stored_events,
            &[completed, canceled, unknown_canceled],
            GUILD,
            now,
        );
        assert_eq!(changes.save, []);
        let closed: Vec<(&str, Status)> = changes
            .close
            .iter()
            .map(|(e, s)| (e.id.as_str(), *s))
            .collect();
        assert_eq!(
            closed,
            [
                ("4", Status::Ended),
                ("5", Status::Canceled),
                ("1", Status::Canceled),
                ("2", Status::Ended),
            ]
        );
    }

    #[test]
    fn announcement_email_has_details_and_one_click_unsubscribe() {
        let event = stored("42", at("2026-10-15T23:00:00Z"), Status::Scheduled);
        let email = email(
            &event,
            Notice::New,
            &recipient(Some("abc"), false),
            "a@example.com",
            SITE,
        );
        assert_eq!(email.to, "a@example.com");
        assert_eq!(email.subject, "New meetup: AI Night");
        assert_eq!(
            email.text,
            "A new fairfieldct.ai meetup is on the calendar.\n\
             \n\
             AI Night\n\
             Thursday, October 15 at 7:00 PM EDT\n\
             Fairfield Library\n\
             \n\
             Demos and pizza.\n\
             \n\
             Details and RSVP: https://www.fairfieldct.ai/events/#event-42\n\
             \n\
             --\n\
             You're getting this because you turned on meetup emails for your fairfieldct.ai account.\n\
             Unsubscribe: https://www.fairfieldct.ai/unsubscribe/#abc\n\
             Email settings: https://www.fairfieldct.ai/account/\n"
        );
        assert_eq!(
            email.headers,
            [
                (
                    "List-Unsubscribe".to_owned(),
                    "<https://www.fairfieldct.ai/api/email/unsubscribe/abc>".to_owned()
                ),
                (
                    "List-Unsubscribe-Post".to_owned(),
                    "List-Unsubscribe=One-Click".to_owned()
                ),
            ]
        );
        assert_eq!(
            email.idempotency_key,
            format!("new-42-{}-u1", event.starts_at)
        );
    }

    #[test]
    fn reminder_and_cancellation_emails_fit_the_recipient() {
        let event = Event {
            location: String::new(),
            ..stored("42", at("2026-10-15T23:00:00Z"), Status::Scheduled)
        };
        let going = email(
            &event,
            Notice::Tomorrow,
            &recipient(None, true),
            "a@example.com",
            SITE,
        );
        assert_eq!(going.subject, "Tomorrow: AI Night");
        assert!(going.text.starts_with("See you tomorrow! You RSVP'd to this meetup.\n\nAI Night\nThursday, October 15 at 7:00 PM EDT\n\nDemos"));
        assert!(going.text.contains(
            "Can't make it? Update your RSVP: https://www.fairfieldct.ai/events/#event-42\n"
        ));
        assert!(going.text.contains("because you RSVP'd"));
        assert_eq!(going.headers, []);

        let subscribed = email(
            &event,
            Notice::Tomorrow,
            &recipient(Some("abc"), false),
            "a@example.com",
            SITE,
        );
        assert!(
            subscribed
                .text
                .starts_with("This fairfieldct.ai meetup is tomorrow.")
        );
        assert_eq!(subscribed.headers.len(), 2);

        // Cancellations are about an RSVP, so they never offer unsubscribing.
        let canceled = email(
            &event,
            Notice::Canceled,
            &recipient(Some("abc"), true),
            "a@example.com",
            SITE,
        );
        assert_eq!(canceled.subject, "Canceled: AI Night");
        assert!(canceled.text.starts_with(
            "Sorry, this meetup on Thursday, October 15 at 7:00 PM EDT has been canceled."
        ));
        assert!(!canceled.text.contains("Demos"));
        assert!(
            canceled
                .text
                .contains("See what else is coming up: https://www.fairfieldct.ai/events/\n")
        );
        assert_eq!(canceled.headers, []);
        assert!(canceled.idempotency_key.starts_with("canceled-42-"));
    }

    /// Addresses by username; `None` means no verified address, and a missing
    /// entry is a lookup failure.
    struct Addresses(Vec<(&'static str, Option<&'static str>)>);

    impl Directory for Addresses {
        fn email<'a>(&'a self, username: &'a str) -> BoxFuture<'a, Option<String>> {
            let result = self
                .0
                .iter()
                .find(|(name, _)| *name == username)
                .map(|(_, address)| address.map(Into::into))
                .ok_or_else(|| "cognito down".into());
            Box::pin(async move { result })
        }
    }

    #[derive(Default)]
    struct Outbox {
        sent: Mutex<Vec<Email>>,
        /// Addresses whose sends fail.
        failing: Vec<&'static str>,
    }

    impl Mailer for Outbox {
        fn send(&self, email: Email) -> BoxFuture<'_, ()> {
            let result = if self.failing.contains(&email.to.as_str()) {
                Err("mail api down".into())
            } else {
                self.sent.lock().unwrap().push(email);
                Ok(())
            };
            Box::pin(async move { result })
        }
    }

    impl Outbox {
        fn sent(&self) -> Vec<(String, String)> {
            let mut sent: Vec<(String, String)> = self
                .sent
                .lock()
                .unwrap()
                .iter()
                .map(|e| (e.to.clone(), e.subject.clone()))
                .collect();
            sent.sort();
            sent
        }
    }

    fn mail(subscribers: Vec<Subscriber>, outbox: &Arc<Outbox>) -> Mail {
        Mail {
            subscribers: Arc::new(MemorySubscribers::with(subscribers)),
            directory: Arc::new(Addresses(vec![
                ("name-u1", Some("u1@example.com")),
                ("name-u2", Some("u2@example.com")),
                ("name-u3", Some("u3@example.com")),
                ("name-unverified", None),
            ])),
            mailer: outbox.clone(),
            site_url: SITE.into(),
        }
    }

    #[tokio::test]
    async fn notices_go_to_the_right_people_once_each() {
        let now = at("2026-10-14T23:00:00Z");
        let event = stored("42", now + 86_400, Status::Scheduled);
        let store = MemoryEvents::with([event.clone()]);
        store.rsvp("42", &attendee("u2"), now).await.unwrap();
        store.rsvp("42", &attendee("u3"), now).await.unwrap();
        let outbox = Arc::new(Outbox::default());
        let mail = mail(
            vec![subscriber("u1"), subscriber("u2"), subscriber("unverified")],
            &outbox,
        );

        assert_eq!(mail.notify(&store, &event, Notice::New).await.unwrap(), 2);
        assert_eq!(
            outbox.sent(),
            [
                ("u1@example.com".into(), "New meetup: AI Night".into()),
                ("u2@example.com".into(), "New meetup: AI Night".into()),
            ]
        );

        outbox.sent.lock().unwrap().clear();
        assert_eq!(
            mail.notify(&store, &event, Notice::Tomorrow).await.unwrap(),
            3
        );
        let sent = outbox.sent.lock().unwrap().clone();
        let to_u2 = sent.iter().find(|e| e.to == "u2@example.com").unwrap();
        // A subscriber who RSVP'd gets one email, as someone who's going,
        // with the unsubscribe link.
        assert!(to_u2.text.starts_with("See you tomorrow!"));
        assert_eq!(to_u2.headers.len(), 2);
        let to_u3 = sent.iter().find(|e| e.to == "u3@example.com").unwrap();
        assert_eq!(to_u3.headers, []);

        outbox.sent.lock().unwrap().clear();
        assert_eq!(
            mail.notify(&store, &event, Notice::Canceled).await.unwrap(),
            2
        );
        assert_eq!(
            outbox.sent(),
            [
                ("u2@example.com".into(), "Canceled: AI Night".into()),
                ("u3@example.com".into(), "Canceled: AI Night".into()),
            ]
        );
    }

    #[tokio::test]
    async fn failed_address_lookups_are_failures() {
        let event = stored("42", at("2026-10-15T23:00:00Z"), Status::Scheduled);
        let store = MemoryEvents::with([event.clone()]);
        let outbox = Arc::new(Outbox::default());
        let mail = mail(vec![subscriber("lookup-fails"), subscriber("u1")], &outbox);
        assert!(mail.notify(&store, &event, Notice::New).await.is_err());
        assert_eq!(outbox.sent().len(), 1);
    }

    #[tokio::test]
    async fn notify_tries_everyone_then_reports_failures() {
        let event = stored("42", at("2026-10-15T23:00:00Z"), Status::Scheduled);
        let store = MemoryEvents::with([event.clone()]);
        let outbox = Arc::new(Outbox {
            failing: vec!["u1@example.com"],
            ..Outbox::default()
        });
        let mail = mail(
            vec![
                subscriber("u1"),
                subscriber("lookup-fails"),
                subscriber("u3"),
            ],
            &outbox,
        );
        assert!(mail.notify(&store, &event, Notice::New).await.is_err());
        assert_eq!(
            outbox.sent(),
            [("u3@example.com".into(), "New meetup: AI Night".into())]
        );
    }

    fn sites(
        outbox: &Arc<Outbox>,
        subscribers: Vec<Subscriber>,
    ) -> (Vec<Site>, Arc<MemoryEvents>, Arc<MemoryEvents>) {
        let prod = Arc::new(MemoryEvents::default());
        let dev = Arc::new(MemoryEvents::default());
        let sites = vec![
            Site {
                events: prod.clone(),
                mail: Some(mail(subscribers, outbox)),
            },
            Site {
                events: dev.clone(),
                mail: None,
            },
        ];
        (sites, prod, dev)
    }

    #[tokio::test]
    async fn run_syncs_every_site_and_emails_one() {
        let run_at = at("2026-10-07T18:15:00Z");
        let outbox = Arc::new(Outbox::default());
        let (sites, prod, dev) = sites(&outbox, vec![subscriber("u1")]);
        let new = discord(run_at - 60, "2026-10-08T18:35:00Z", 1);

        let summary = run(&sites, std::slice::from_ref(&new), GUILD, run_at, INTERVAL).await;
        assert_eq!(
            summary,
            Summary {
                saved: 2,
                closed: 0,
                emailed: 1,
                failures: 0
            }
        );
        assert_eq!(prod.list().await.unwrap().len(), 1);
        assert_eq!(dev.list().await.unwrap().len(), 1);
        assert_eq!(
            outbox.sent(),
            [("u1@example.com".into(), "New meetup: AI Night".into())]
        );

        // The day-before window: one reminder, nothing re-saved.
        outbox.sent.lock().unwrap().clear();
        let day_before = at("2026-10-07T18:15:00Z") + INTERVAL;
        let summary = run(
            &sites,
            std::slice::from_ref(&new),
            GUILD,
            day_before,
            INTERVAL,
        )
        .await;
        assert_eq!(
            summary,
            Summary {
                saved: 0,
                closed: 0,
                emailed: 1,
                failures: 0
            }
        );
        assert_eq!(
            outbox.sent(),
            [("u1@example.com".into(), "Tomorrow: AI Night".into())]
        );

        // Discord drops it before it starts: canceled everywhere, and the
        // RSVP'd member hears about it from prod only.
        outbox.sent.lock().unwrap().clear();
        prod.rsvp(&new.id, &attendee("u3"), day_before)
            .await
            .unwrap();
        let later = day_before + 3 * INTERVAL;
        let summary = run(&sites, &[], GUILD, later, INTERVAL).await;
        assert_eq!(
            summary,
            Summary {
                saved: 0,
                closed: 2,
                emailed: 1,
                failures: 0
            }
        );
        assert_eq!(
            outbox.sent(),
            [("u3@example.com".into(), "Canceled: AI Night".into())]
        );
        assert_eq!(
            dev.get(&new.id).await.unwrap().unwrap().status,
            Status::Canceled
        );
        assert_eq!(
            prod.get(&new.id).await.unwrap().unwrap().status,
            Status::Canceled
        );
    }

    #[tokio::test]
    async fn run_counts_sync_and_email_failures() {
        let run_at = at("2026-10-07T18:15:00Z");
        let outbox = Arc::new(Outbox {
            failing: vec!["u1@example.com"],
            ..Outbox::default()
        });
        let (mut sites, _, _) = sites(&outbox, vec![subscriber("u1")]);
        sites[1].events = Arc::new(Unavailable);
        let new = discord(run_at - 60, "2026-10-20T18:00:00Z", 1);
        let summary = run(&sites, &[new], GUILD, run_at, INTERVAL).await;
        // Dev's sync and prod's announcement both failed; prod still synced.
        assert_eq!(
            summary,
            Summary {
                saved: 1,
                closed: 0,
                emailed: 0,
                failures: 2
            }
        );
    }

    /// An event store that's down.
    struct Unavailable;

    impl EventStore for Unavailable {
        fn list(&self) -> shared::links::BoxFuture<'_, Result<Vec<Event>, shared::Error>> {
            Box::pin(async { Err("down".into()) })
        }
        fn get<'a>(
            &'a self,
            _: &'a str,
        ) -> shared::links::BoxFuture<'a, Result<Option<Event>, shared::Error>> {
            Box::pin(async { Err("down".into()) })
        }
        fn save<'a>(
            &'a self,
            _: &'a Event,
        ) -> shared::links::BoxFuture<'a, Result<(), shared::Error>> {
            Box::pin(async { Err("down".into()) })
        }
        fn close<'a>(
            &'a self,
            _: &'a str,
            _: Status,
        ) -> shared::links::BoxFuture<'a, Result<bool, shared::Error>> {
            Box::pin(async { Err("down".into()) })
        }
        fn rsvp<'a>(
            &'a self,
            _: &'a str,
            _: &'a Attendee,
            _: i64,
        ) -> shared::links::BoxFuture<'a, Result<u32, shared::events::RsvpError>> {
            Box::pin(async { Err(shared::events::RsvpError::Store("down".into())) })
        }
        fn cancel_rsvp<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
        ) -> shared::links::BoxFuture<'a, Result<u32, shared::events::RsvpError>> {
            Box::pin(async { Err(shared::events::RsvpError::Store("down".into())) })
        }
        fn rsvps_by_user<'a>(
            &'a self,
            _: &'a str,
        ) -> shared::links::BoxFuture<'a, Result<Vec<String>, shared::Error>> {
            Box::pin(async { Err("down".into()) })
        }
        fn attendees<'a>(
            &'a self,
            _: &'a str,
        ) -> shared::links::BoxFuture<'a, Result<Vec<Attendee>, shared::Error>> {
            Box::pin(async { Err("down".into()) })
        }
    }

    #[tokio::test]
    async fn failed_cancellation_emails_keep_the_event_open_for_a_retry() {
        let run_at = at("2026-10-07T18:15:00Z");
        let outbox = Arc::new(Outbox {
            failing: vec!["u3@example.com"],
            ..Outbox::default()
        });
        let (sites, prod, _) = sites(&outbox, vec![]);
        let event = stored("42", run_at + 86_400, Status::Scheduled);
        prod.save(&event).await.unwrap();
        prod.rsvp("42", &attendee("u3"), run_at).await.unwrap();

        let summary = run(&sites, &[], GUILD, run_at, INTERVAL).await;
        assert_eq!(
            summary,
            Summary {
                saved: 0,
                closed: 0,
                emailed: 0,
                failures: 1
            }
        );
        assert_eq!(
            prod.get("42").await.unwrap().unwrap().status,
            Status::Scheduled
        );
    }
}
