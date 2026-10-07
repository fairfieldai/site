//! Meetup announcements and reminders for #announcements.
//!
//! The job runs on a fixed interval and covers exactly one interval per run,
//! keyed to its scheduled time rather than when it actually starts. Every
//! announcement or reminder therefore falls into exactly one run and posts
//! once, with no stored state.

use lambda_http::{Error, tracing};

use crate::events::{Announcer, EventSource, ScheduledEvent};

const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reminder {
    /// The event was created during the run's interval.
    New,
    /// The event starts in about a week.
    Week,
    /// The event starts in about a day.
    Day,
    /// The event starts in about an hour.
    Soon,
}

impl Reminder {
    const BEFORE_START: [(Self, i64); 3] =
        [(Self::Week, 7 * DAY), (Self::Day, DAY), (Self::Soon, HOUR)];

    fn heading(self, name: &str) -> String {
        match self {
            Self::New => format!("New meetup: {name}"),
            Self::Week => format!("One week until {name}"),
            Self::Day => format!("Tomorrow: {name}"),
            Self::Soon => format!("Starting in an hour: {name}"),
        }
    }
}

/// Posts due for the run scheduled at `run_at` (Unix seconds), covering
/// `interval` seconds.
///
/// A new-meetup post is due for events created in `[run_at - interval, run_at)`.
/// A reminder `offset` before the start is due for events starting in
/// `[run_at + offset, run_at + offset + interval)`.
#[must_use]
pub fn due(
    events: &[ScheduledEvent],
    run_at: i64,
    interval: i64,
) -> Vec<(Reminder, &ScheduledEvent)> {
    let mut posts = Vec::new();
    for event in events.iter().filter(|event| event.is_scheduled()) {
        if event
            .created_at()
            .is_some_and(|created| (run_at - interval..run_at).contains(&created))
        {
            posts.push((Reminder::New, event));
        }
        let Some(start) = event.starts_at() else {
            continue;
        };
        for (reminder, offset) in Reminder::BEFORE_START {
            if (run_at + offset..run_at + offset + interval).contains(&start) {
                posts.push((reminder, event));
            }
        }
    }
    posts
}

/// Posts every announcement and reminder due for this run, returning how many
/// were posted.
///
/// # Errors
///
/// If the events can't be read or a post fails. Posts before the failure stay
/// posted.
pub async fn run(
    events: &dyn EventSource,
    announcer: &dyn Announcer,
    guild_id: &str,
    run_at: i64,
    interval: i64,
) -> Result<usize, Error> {
    let events = events.scheduled_events().await?;
    let posts = due(&events, run_at, interval);
    for (reminder, event) in &posts {
        tracing::info!(event = event.id, reminder = ?reminder, "posting meetup reminder");
        announcer
            .announce(event.message(&reminder.heading(&event.name), guild_id))
            .await?;
    }
    Ok(posts.len())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::events::{BoxFuture, EntityMetadata, parse_rfc3339};

    const INTERVAL: i64 = 15 * 60;
    const DISCORD_EPOCH: i64 = 1_420_070_400;

    fn at(timestamp: &str) -> i64 {
        parse_rfc3339(timestamp).unwrap()
    }

    /// An event created at `created` (Unix seconds) and starting at `start`.
    fn event(created: i64, start: &str) -> ScheduledEvent {
        let snowflake = u64::try_from((created - DISCORD_EPOCH) * 1000).unwrap() << 22;
        ScheduledEvent {
            id: snowflake.to_string(),
            name: "AI Night".into(),
            description: None,
            scheduled_start_time: start.into(),
            status: 1,
            entity_type: 3,
            channel_id: None,
            entity_metadata: Some(EntityMetadata {
                location: Some("Fairfield Library".into()),
            }),
        }
    }

    fn kinds(posts: &[(Reminder, &ScheduledEvent)]) -> Vec<Reminder> {
        posts.iter().map(|(reminder, _)| *reminder).collect()
    }

    #[test]
    fn new_events_are_announced_in_the_interval_they_were_created() {
        let run_at = at("2026-10-07T18:15:00Z");
        let created_in = event(run_at - 60, "2026-11-20T23:00:00Z");
        let created_at_start = event(run_at - INTERVAL, "2026-11-20T23:00:00Z");
        let created_before = event(run_at - INTERVAL - 1, "2026-11-20T23:00:00Z");
        let created_at_run = event(run_at, "2026-11-20T23:00:00Z");
        assert_eq!(
            kinds(&due(&[created_in], run_at, INTERVAL)),
            [Reminder::New]
        );
        assert_eq!(
            kinds(&due(&[created_at_start], run_at, INTERVAL)),
            [Reminder::New]
        );
        assert_eq!(
            kinds(&due(&[created_before], run_at, INTERVAL)),
            Vec::<Reminder>::new()
        );
        assert_eq!(
            kinds(&due(&[created_at_run], run_at, INTERVAL)),
            Vec::<Reminder>::new()
        );
    }

    #[test]
    fn reminders_fall_in_exactly_one_run() {
        let start = "2026-10-15T23:00:00Z";
        let meetup = event(at("2026-09-01T00:00:00Z"), start);
        let first_run = at(start) - 8 * DAY;
        let mut seen = Vec::new();
        for step in 0..=(8 * DAY / INTERVAL) {
            let run_at = first_run + step * INTERVAL;
            seen.extend(kinds(&due(std::slice::from_ref(&meetup), run_at, INTERVAL)));
        }
        assert_eq!(seen, [Reminder::Week, Reminder::Day, Reminder::Soon]);
    }

    #[test]
    fn reminder_windows_are_start_inclusive_and_end_exclusive() {
        let run_at = at("2026-10-14T23:00:00Z");
        let starts_on_edge = event(at("2026-09-01T00:00:00Z"), "2026-10-15T23:00:00Z");
        let starts_at_window_end = event(at("2026-09-01T00:00:00Z"), "2026-10-15T23:15:00Z");
        assert_eq!(
            kinds(&due(&[starts_on_edge], run_at, INTERVAL)),
            [Reminder::Day]
        );
        assert_eq!(
            kinds(&due(&[starts_at_window_end], run_at, INTERVAL)),
            Vec::<Reminder>::new()
        );
    }

    #[test]
    fn only_scheduled_events_get_posts() {
        let run_at = at("2026-10-15T22:00:00Z");
        let mut canceled = event(run_at - 60, "2026-10-15T23:00:00Z");
        canceled.status = 4;
        let mut active = event(run_at - 60, "2026-10-15T23:00:00Z");
        active.status = 2;
        assert_eq!(
            kinds(&due(&[canceled, active], run_at, INTERVAL)),
            Vec::<Reminder>::new()
        );
    }

    #[test]
    fn an_event_created_an_hour_out_gets_both_posts() {
        let run_at = at("2026-10-15T22:00:00Z");
        let last_minute = event(run_at - 60, "2026-10-15T23:05:00Z");
        assert_eq!(
            kinds(&due(&[last_minute], run_at, INTERVAL)),
            [Reminder::New, Reminder::Soon]
        );
    }

    #[test]
    fn events_without_a_start_time_can_still_be_announced() {
        let run_at = at("2026-10-15T22:00:00Z");
        let unknown = event(run_at - 60, "someday");
        assert_eq!(kinds(&due(&[unknown], run_at, INTERVAL)), [Reminder::New]);
    }

    #[test]
    fn headings_name_the_meetup() {
        assert_eq!(Reminder::New.heading("X"), "New meetup: X");
        assert_eq!(Reminder::Week.heading("X"), "One week until X");
        assert_eq!(Reminder::Day.heading("X"), "Tomorrow: X");
        assert_eq!(Reminder::Soon.heading("X"), "Starting in an hour: X");
    }

    struct Events(Result<Vec<ScheduledEvent>, String>);

    impl EventSource for Events {
        fn scheduled_events(&self) -> BoxFuture<'_, Vec<ScheduledEvent>> {
            let result = self.0.clone().map_err(Error::from);
            Box::pin(async move { result })
        }
    }

    #[derive(Default)]
    struct Posts {
        sent: Mutex<Vec<String>>,
        fail: bool,
    }

    impl Announcer for Posts {
        fn announce(&self, content: String) -> BoxFuture<'_, ()> {
            let result = if self.fail {
                Err("webhook down".into())
            } else {
                self.sent.lock().unwrap().push(content);
                Ok(())
            };
            Box::pin(async move { result })
        }
    }

    #[tokio::test]
    async fn run_posts_each_due_reminder() {
        let run_at = at("2026-10-14T23:00:00Z");
        let source = Events(Ok(vec![event(
            at("2026-09-01T00:00:00Z"),
            "2026-10-15T23:05:00Z",
        )]));
        let posts = Posts::default();
        assert_eq!(
            run(&source, &posts, "9", run_at, INTERVAL).await.unwrap(),
            1
        );
        let sent = posts.sent.lock().unwrap();
        assert!(sent[0].starts_with("**Tomorrow: AI Night**\n<t:"));
        assert!(sent[0].contains("Where: Fairfield Library"));
    }

    #[tokio::test]
    async fn run_with_nothing_due_posts_nothing() {
        let posts = Posts::default();
        let source = Events(Ok(vec![]));
        assert_eq!(run(&source, &posts, "9", 0, INTERVAL).await.unwrap(), 0);
        assert!(posts.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn run_fails_when_events_or_posts_fail() {
        let posts = Posts::default();
        let unavailable = Events(Err("discord down".into()));
        assert!(run(&unavailable, &posts, "9", 0, INTERVAL).await.is_err());

        let run_at = at("2026-10-14T23:00:00Z");
        let source = Events(Ok(vec![event(
            at("2026-09-01T00:00:00Z"),
            "2026-10-15T23:00:00Z",
        )]));
        let failing = Posts {
            fail: true,
            ..Posts::default()
        };
        assert!(run(&source, &failing, "9", run_at, INTERVAL).await.is_err());
    }
}
