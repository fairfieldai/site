//! The meetup calendar feed (RFC 5545), for subscribing from calendar apps.

use shared::events::{Event, Status};
use shared::time::ical_utc;

/// Longest content line in octets, not counting the CRLF.
const LINE_LIMIT: usize = 75;

/// Escapes a TEXT value: backslashes, semicolons, commas, and newlines.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            ';' => escaped.push_str("\\;"),
            ',' => escaped.push_str("\\,"),
            '\n' => escaped.push_str("\\n"),
            '\r' => {}
            c => escaped.push(c),
        }
    }
    escaped
}

/// Appends a content line, folded every 75 octets without splitting a
/// character.
fn line(calendar: &mut String, content: &str) {
    let mut length = 0;
    for c in content.chars() {
        if length + c.len_utf8() > LINE_LIMIT {
            calendar.push_str("\r\n ");
            // The leading space counts toward the continuation line.
            length = 1;
        }
        calendar.push(c);
        length += c.len_utf8();
    }
    calendar.push_str("\r\n");
}

/// The calendar for `events`, linking each to its place on `site_url`.
#[must_use]
pub fn calendar(events: &[Event], site_url: &str, now: i64) -> String {
    let mut calendar = String::new();
    for content in [
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "PRODID:-//fairfieldct.ai//Meetups//EN",
        "CALSCALE:GREGORIAN",
        "METHOD:PUBLISH",
        "X-WR-CALNAME:fairfieldct.ai meetups",
        "REFRESH-INTERVAL;VALUE=DURATION:PT1H",
        "X-PUBLISHED-TTL:PT1H",
    ] {
        line(&mut calendar, content);
    }
    let stamp = ical_utc(now);
    for event in events {
        let url = format!("{site_url}/events/#event-{}", event.id);
        let description = if event.description.is_empty() {
            url.clone()
        } else {
            format!("{}\n\n{url}", event.description)
        };
        let status = match event.status {
            Status::Canceled => "CANCELLED",
            Status::Scheduled | Status::Active | Status::Ended => "CONFIRMED",
        };
        line(&mut calendar, "BEGIN:VEVENT");
        line(&mut calendar, &format!("UID:{}@fairfieldct.ai", event.id));
        line(&mut calendar, &format!("DTSTAMP:{stamp}"));
        line(
            &mut calendar,
            &format!("DTSTART:{}", ical_utc(event.starts_at)),
        );
        line(&mut calendar, &format!("DTEND:{}", ical_utc(event.ends_at)));
        line(&mut calendar, &format!("SUMMARY:{}", escape(&event.name)));
        line(
            &mut calendar,
            &format!("DESCRIPTION:{}", escape(&description)),
        );
        if !event.location.is_empty() {
            line(
                &mut calendar,
                &format!("LOCATION:{}", escape(&event.location)),
            );
        }
        line(&mut calendar, &format!("URL:{url}"));
        line(&mut calendar, &format!("STATUS:{status}"));
        line(&mut calendar, "END:VEVENT");
    }
    line(&mut calendar, "END:VCALENDAR");
    calendar
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> Event {
        Event {
            id: "42".into(),
            name: "AI Night; demos, too".into(),
            description: "Bring a laptop.\nPizza provided.".into(),
            starts_at: 1_792_105_200,
            ends_at: 1_792_112_400,
            location: "Fairfield Library".into(),
            url: "https://discord.com/events/9/42".into(),
            status: Status::Scheduled,
            rsvps: 0,
        }
    }

    #[test]
    fn escapes_text_values() {
        assert_eq!(escape(r"a\b;c,d"), r"a\\b\;c\,d");
        assert_eq!(escape("one\r\ntwo"), r"one\ntwo");
    }

    #[test]
    fn folds_long_lines_on_character_boundaries() {
        let mut folded = String::new();
        line(&mut folded, &"a".repeat(80));
        assert_eq!(
            folded,
            format!("{}\r\n {}\r\n", "a".repeat(75), "a".repeat(5))
        );

        // A three-byte character that would cross the limit moves to the
        // next line whole.
        let mut folded = String::new();
        line(&mut folded, &format!("{}€", "a".repeat(73)));
        assert_eq!(folded, format!("{}\r\n €\r\n", "a".repeat(73)));

        let mut folded = String::new();
        line(&mut folded, &"€".repeat(60));
        for physical in folded.split("\r\n") {
            assert!(physical.len() <= LINE_LIMIT, "{physical:?}");
        }
        assert_eq!(
            folded.replace("\r\n ", ""),
            format!("{}\r\n", "€".repeat(60))
        );
    }

    #[test]
    fn calendar_lists_each_event() {
        let canceled = Event {
            id: "43".into(),
            status: Status::Canceled,
            description: String::new(),
            location: String::new(),
            ..event()
        };
        let calendar = calendar(&[event(), canceled], "https://www.fairfieldct.ai", 0);
        assert!(calendar.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"));
        assert!(calendar.ends_with("END:VEVENT\r\nEND:VCALENDAR\r\n"));
        let unfolded = calendar.replace("\r\n ", "");
        for expected in [
            "UID:42@fairfieldct.ai\r\n",
            "DTSTAMP:19700101T000000Z\r\n",
            "DTSTART:20261015T230000Z\r\n",
            "DTEND:20261016T010000Z\r\n",
            "SUMMARY:AI Night\\; demos\\, too\r\n",
            "DESCRIPTION:Bring a laptop.\\nPizza provided.\\n\\nhttps://www.fairfieldct.ai/events/#event-42\r\n",
            "LOCATION:Fairfield Library\r\n",
            "URL:https://www.fairfieldct.ai/events/#event-42\r\n",
            "STATUS:CONFIRMED\r\n",
            "UID:43@fairfieldct.ai\r\n",
            "DESCRIPTION:https://www.fairfieldct.ai/events/#event-43\r\n",
            "STATUS:CANCELLED\r\n",
        ] {
            assert!(unfolded.contains(expected), "missing {expected:?}");
        }
        // The canceled event has no location line.
        assert_eq!(unfolded.matches("LOCATION:").count(), 1);
        assert!(!calendar.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn empty_calendar_is_valid() {
        let calendar = calendar(&[], "https://www.fairfieldct.ai", 0);
        assert!(!calendar.contains("VEVENT"));
        assert!(calendar.ends_with("END:VCALENDAR\r\n"));
    }
}
