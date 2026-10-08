//! Timestamps in Unix seconds: parsing Discord's RFC 3339 times, and writing
//! UTC calendar stamps and Fairfield (US Eastern) local times.

use std::time::{SystemTime, UNIX_EPOCH};

const DAY: i64 = 86_400;
const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// The current time in Unix seconds.
#[must_use]
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
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
        days * DAY + number(hour, 23)? * 3600 + number(minute, 59)? * 60 + number(second, 60)?
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

/// The `(year, month, day)` of a day count since 1970-01-01 (Howard Hinnant's
/// `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// 0 for Sunday through 6 for Saturday.
fn weekday(days: i64) -> i64 {
    // 1970-01-01 was a Thursday.
    (days + 4).rem_euclid(7)
}

/// A UTC timestamp in iCalendar's form, e.g. `20261015T230000Z`.
#[must_use]
pub fn ical_utc(seconds: i64) -> String {
    let (year, month, day) = civil_from_days(seconds.div_euclid(DAY));
    let time = seconds.rem_euclid(DAY);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        time / 3600,
        time % 3600 / 60,
        time % 60
    )
}

/// The day number of the `nth` (1-based) Sunday of a month.
fn nth_sunday(year: i64, month: i64, nth: i64) -> Option<i64> {
    let first = days_from_civil(year, month, 1)?;
    Some(first + (7 - weekday(first)) % 7 + 7 * (nth - 1))
}

/// Whether US Eastern daylight time is in effect at `seconds`: from 2:00 a.m.
/// local on the second Sunday in March to 2:00 a.m. local on the first Sunday
/// in November.
fn eastern_daylight(seconds: i64) -> bool {
    let (year, _, _) = civil_from_days(seconds.div_euclid(DAY));
    let (Some(march), Some(november)) = (nth_sunday(year, 3, 2), nth_sunday(year, 11, 1)) else {
        return false;
    };
    // 2:00 EST is 07:00 UTC; 2:00 EDT is 06:00 UTC.
    (march * DAY + 7 * 3600..november * DAY + 6 * 3600).contains(&seconds)
}

/// A Fairfield, Connecticut local time for people, e.g.
/// `Thursday, October 15 at 7:00 PM EDT`.
#[must_use]
pub fn eastern(seconds: i64) -> String {
    let (offset, zone) = if eastern_daylight(seconds) {
        (-4 * 3600, "EDT")
    } else {
        (-5 * 3600, "EST")
    };
    let local = seconds + offset;
    let days = local.div_euclid(DAY);
    let (_, month, day) = civil_from_days(days);
    let time = local.rem_euclid(DAY);
    let (hour, minute) = (time / 3600, time % 3600 / 60);
    let (hour12, meridiem) = match hour {
        0 => (12, "AM"),
        1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        _ => (hour - 12, "PM"),
    };
    // Both indexes are in range: weekday is 0..7 and month is 1..=12.
    let weekday = WEEKDAYS[usize::try_from(weekday(days)).unwrap_or(0)];
    let month = MONTHS[usize::try_from(month - 1).unwrap_or(0)];
    format!("{weekday}, {month} {day} at {hour12}:{minute:02} {meridiem} {zone}")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            parse_rfc3339("2026-10-15T23:45:17Z"),
            Some(1_792_105_200 + 45 * 60 + 17)
        );
        assert_eq!(
            parse_rfc3339("2026-10-16T05:15:00+05:30"),
            Some(1_792_105_200 + 45 * 60)
        );
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
            "2026-10-15T+1:00:00Z",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
    }

    #[test]
    fn civil_dates_round_trip() {
        for days in [-719_468, -1, 0, 1, 10_957, 11_016, 20_741, 2_932_896] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), Some(days), "{days}");
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn civil_dates_round_trip_across_centuries() {
        // 1600-03-01 through 2500, stepping by a prime so every month and
        // century rule is hit.
        let mut days = -135_080;
        while days < 193_000 {
            let (year, month, day) = civil_from_days(days);
            assert!(
                (1..=12).contains(&month) && (1..=31).contains(&day),
                "{days}"
            );
            assert_eq!(days_from_civil(year, month, day), Some(days), "{days}");
            days += 7;
        }
        assert_eq!(civil_from_days(47_541), (2100, 3, 1));
        assert_eq!(civil_from_days(47_540), (2100, 2, 28));
        assert_eq!(civil_from_days(-25_508), (1900, 3, 1));
    }

    #[test]
    fn now_is_the_current_time() {
        let now = now();
        assert!(now > 1_790_000_000, "{now}");
        assert!(now < 4_102_444_800, "{now}");
    }

    #[test]
    fn writes_ical_utc_stamps() {
        assert_eq!(ical_utc(0), "19700101T000000Z");
        assert_eq!(ical_utc(1_792_105_200), "20261015T230000Z");
        assert_eq!(ical_utc(1_709_208_059), "20240229T120059Z");
        assert_eq!(ical_utc(-1), "19691231T235959Z");
    }

    #[test]
    fn finds_dst_sundays() {
        let date = |days| civil_from_days(days);
        assert_eq!(date(nth_sunday(2026, 3, 2).unwrap()), (2026, 3, 8));
        assert_eq!(date(nth_sunday(2026, 11, 1).unwrap()), (2026, 11, 1));
        assert_eq!(date(nth_sunday(2027, 3, 2).unwrap()), (2027, 3, 14));
        assert_eq!(date(nth_sunday(2027, 11, 1).unwrap()), (2027, 11, 7));
    }

    #[test]
    fn eastern_time_follows_daylight_saving() {
        let at = |timestamp| eastern(parse_rfc3339(timestamp).unwrap());
        assert_eq!(
            at("2026-10-15T23:00:00Z"),
            "Thursday, October 15 at 7:00 PM EDT"
        );
        assert_eq!(
            at("2026-12-01T17:30:00Z"),
            "Tuesday, December 1 at 12:30 PM EST"
        );
        assert_eq!(
            at("2026-01-01T05:00:00Z"),
            "Thursday, January 1 at 12:00 AM EST"
        );
        // Spring forward: 1:59 EST, then 3:00 EDT.
        assert_eq!(at("2026-03-08T06:59:00Z"), "Sunday, March 8 at 1:59 AM EST");
        assert_eq!(at("2026-03-08T07:00:00Z"), "Sunday, March 8 at 3:00 AM EDT");
        // Fall back: 1:59 EDT, then 1:00 EST.
        assert_eq!(
            at("2026-11-01T05:59:00Z"),
            "Sunday, November 1 at 1:59 AM EDT"
        );
        assert_eq!(
            at("2026-11-01T06:00:00Z"),
            "Sunday, November 1 at 1:00 AM EST"
        );
        // A UTC date after local midnight is still the previous local day.
        assert_eq!(
            at("2026-10-16T02:30:00Z"),
            "Thursday, October 15 at 10:30 PM EDT"
        );
    }
}
