//! A five-field cron expression, just enough to say "every night at three".
//!
//! Written by hand rather than pulled in from a crate because the only thing
//! needed here is "when is the next matching minute", and the obvious crates
//! bring a full date/time library with them. The CLI already hand-rolls its
//! civil-date maths for the same reason; this keeps the agent a small static
//! binary.
//!
//! Deliberate limits: no seconds field, no `@daily` aliases, no timezone
//! database. Times are matched against UTC shifted by a fixed offset from the
//! config, so daylight saving is not handled — a scheduled scan is allowed to
//! land an hour off twice a year.

use std::fmt;

use serde::{Deserialize, Deserializer};

/// Seconds in a minute, the resolution the scheduler works at.
const MINUTE: i64 = 60;

/// How far ahead `next_after` is willing to look before giving up. Four years
/// covers the worst legitimate case (29 February on a specific weekday) with
/// room to spare; beyond that the expression matches nothing.
const SEARCH_LIMIT_DAYS: i64 = 4 * 366;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    minutes: u64,
    hours: u64,
    /// Days of month, bit 1..=31.
    days: u64,
    /// Months, bit 1..=12.
    months: u64,
    /// Days of week, bit 0..=6 with 0 = Sunday.
    weekdays: u64,
    /// Vixie cron's rule: when *both* the day-of-month and day-of-week fields
    /// are restricted, a day matches if *either* does. Storing whether each was
    /// `*` is the only way to reproduce that.
    dom_restricted: bool,
    dow_restricted: bool,
    source: String,
}

impl Schedule {
    pub fn parse(expr: &str) -> Result<Self, CronError> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(CronError::new(
                expr,
                format!(
                    "expected 5 fields (minute hour day-of-month month day-of-week), found {}",
                    fields.len()
                ),
            ));
        }

        let minutes = parse_field(fields[0], 0, 59, expr, "minute")?;
        let hours = parse_field(fields[1], 0, 23, expr, "hour")?;
        let days = parse_field(fields[2], 1, 31, expr, "day-of-month")?;
        let months = parse_field(fields[3], 1, 12, expr, "month")?;
        let weekdays = parse_weekday_field(fields[4], expr)?;

        Ok(Schedule {
            minutes,
            hours,
            days,
            months,
            weekdays,
            dom_restricted: fields[2] != "*",
            dow_restricted: fields[4] != "*",
            source: expr.trim().to_string(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// The first matching minute strictly after `after`, in the same shifted
    /// clock the caller used. Returns `None` when nothing matches within the
    /// search window (e.g. `0 0 30 2 *`, the 30th of February).
    pub fn next_after(&self, after: i64) -> Option<i64> {
        // Start at the top of the next minute so a schedule never fires twice
        // for the same minute.
        let start = (after / MINUTE + 1) * MINUTE;
        let start_day = start.div_euclid(86_400);
        let start_secs_of_day = start.rem_euclid(86_400);

        for day_offset in 0..SEARCH_LIMIT_DAYS {
            let day = start_day + day_offset;
            if !self.matches_day(day) {
                continue;
            }
            // Only the first candidate day is entered part-way through.
            let from_secs = if day_offset == 0 {
                start_secs_of_day
            } else {
                0
            };
            if let Some(secs) = self.first_time_of_day_from(from_secs) {
                return Some(day * 86_400 + secs);
            }
        }
        None
    }

    fn matches_day(&self, days_from_epoch: i64) -> bool {
        let (_, month, dom) = civil_from_days(days_from_epoch);
        if !bit(self.months, month) {
            return false;
        }
        let dow = weekday_from_days(days_from_epoch);
        let dom_hit = bit(self.days, dom);
        let dow_hit = bit(self.weekdays, dow);

        match (self.dom_restricted, self.dow_restricted) {
            // Both narrowed: cron unions them rather than intersecting.
            (true, true) => dom_hit || dow_hit,
            (true, false) => dom_hit,
            (false, true) => dow_hit,
            (false, false) => true,
        }
    }

    fn first_time_of_day_from(&self, from_secs: i64) -> Option<i64> {
        let from_hour = (from_secs / 3600) as u32;
        let from_minute = ((from_secs % 3600) / 60) as u32;
        for hour in from_hour..24 {
            if !bit(self.hours, hour) {
                continue;
            }
            let first_minute = if hour == from_hour { from_minute } else { 0 };
            for minute in first_minute..60 {
                if bit(self.minutes, minute) {
                    return Some(hour as i64 * 3600 + minute as i64 * 60);
                }
            }
        }
        None
    }
}

impl fmt::Display for Schedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

impl<'de> Deserialize<'de> for Schedule {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Schedule::parse(&raw).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronError {
    expr: String,
    reason: String,
}

impl CronError {
    fn new(expr: &str, reason: impl Into<String>) -> Self {
        CronError {
            expr: expr.trim().to_string(),
            reason: reason.into(),
        }
    }
}

impl fmt::Display for CronError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad cron expression {:?}: {}", self.expr, self.reason)
    }
}

impl std::error::Error for CronError {}

fn bit(mask: u64, n: u32) -> bool {
    mask & (1u64 << n) != 0
}

/// `*`, `*/n`, `a`, `a-b`, `a-b/n`, and comma-separated lists of those.
fn parse_field(field: &str, min: u32, max: u32, expr: &str, name: &str) -> Result<u64, CronError> {
    let mut mask = 0u64;
    for part in field.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(CronError::new(expr, format!("empty entry in {name} field")));
        }
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => {
                let step: u32 = s.parse().map_err(|_| {
                    CronError::new(expr, format!("step {s:?} in {name} field is not a number"))
                })?;
                if step == 0 {
                    return Err(CronError::new(expr, format!("step of 0 in {name} field")));
                }
                (r, step)
            }
            None => (part, 1),
        };

        let (lo, hi) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            (
                parse_number(a, min, max, expr, name)?,
                parse_number(b, min, max, expr, name)?,
            )
        } else {
            let n = parse_number(range, min, max, expr, name)?;
            // A bare number with a step means "from n to the end of the range",
            // which is what cron does for `5/10`.
            if step > 1 {
                (n, max)
            } else {
                (n, n)
            }
        };

        if lo > hi {
            return Err(CronError::new(
                expr,
                format!("range {lo}-{hi} runs backwards in {name} field"),
            ));
        }
        let mut value = lo;
        while value <= hi {
            mask |= 1u64 << value;
            value += step;
        }
    }
    Ok(mask)
}

/// Day-of-week, where both 0 and 7 mean Sunday as in every other cron.
fn parse_weekday_field(field: &str, expr: &str) -> Result<u64, CronError> {
    let mask = parse_field(field, 0, 7, expr, "day-of-week")?;
    // Fold 7 down onto 0 so matching only has to look at bits 0..=6.
    if bit(mask, 7) {
        Ok((mask & !(1u64 << 7)) | 1)
    } else {
        Ok(mask)
    }
}

fn parse_number(s: &str, min: u32, max: u32, expr: &str, name: &str) -> Result<u32, CronError> {
    let n: u32 = s
        .trim()
        .parse()
        .map_err(|_| CronError::new(expr, format!("{s:?} in {name} field is not a number")))?;
    if n < min || n > max {
        return Err(CronError::new(
            expr,
            format!("{n} is outside {min}-{max} in {name} field"),
        ));
    }
    Ok(n)
}

/// Howard Hinnant's civil_from_days. Same algorithm the CLI uses to format
/// timestamps without a date library.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 0 = Sunday. 1970-01-01 was a Thursday, hence the +4.
fn weekday_from_days(days_from_epoch: i64) -> u32 {
    (days_from_epoch + 4).rem_euclid(7) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unix seconds for a UTC civil datetime, so tests read as dates.
    fn at(y: i64, m: u32, d: u32, hh: i64, mm: i64) -> i64 {
        days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60
    }

    fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
        let doy = (153 * mp + 2) / 5 + d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    #[test]
    fn the_civil_date_helpers_agree_with_each_other() {
        for (y, m, d) in [
            (1970, 1, 1),
            (2000, 2, 29),
            (2026, 9, 7),
            (2024, 12, 31),
            (1999, 3, 1),
        ] {
            let days = days_from_civil(y, m, d);
            assert_eq!(civil_from_days(days), (y, m, d), "{y}-{m}-{d}");
        }
        // 1970-01-01 was a Thursday (4), 2026-09-07 a Monday (1).
        assert_eq!(weekday_from_days(0), 4);
        assert_eq!(weekday_from_days(days_from_civil(2026, 9, 7)), 1);
    }

    #[test]
    fn a_daily_schedule_fires_at_the_right_minute() {
        let s = Schedule::parse("0 3 * * *").unwrap();
        let now = at(2026, 9, 7, 12, 0);
        assert_eq!(s.next_after(now), Some(at(2026, 9, 8, 3, 0)));
        // Just before it fires, it is still today.
        assert_eq!(
            s.next_after(at(2026, 9, 7, 2, 59)),
            Some(at(2026, 9, 7, 3, 0))
        );
    }

    #[test]
    fn a_schedule_never_returns_the_minute_it_was_given() {
        let s = Schedule::parse("0 3 * * *").unwrap();
        let exactly_now = at(2026, 9, 7, 3, 0);
        assert_eq!(s.next_after(exactly_now), Some(at(2026, 9, 8, 3, 0)));
    }

    #[test]
    fn steps_and_lists_are_understood() {
        let s = Schedule::parse("*/15 * * * *").unwrap();
        let base = at(2026, 9, 7, 10, 0);
        assert_eq!(s.next_after(base), Some(at(2026, 9, 7, 10, 15)));
        assert_eq!(
            s.next_after(at(2026, 9, 7, 10, 50)),
            Some(at(2026, 9, 7, 11, 0))
        );

        let s = Schedule::parse("0 2,14 * * *").unwrap();
        assert_eq!(
            s.next_after(at(2026, 9, 7, 3, 0)),
            Some(at(2026, 9, 7, 14, 0))
        );
    }

    #[test]
    fn ranges_with_steps_work() {
        let s = Schedule::parse("0 9-17/4 * * *").unwrap();
        assert_eq!(
            s.next_after(at(2026, 9, 7, 0, 0)),
            Some(at(2026, 9, 7, 9, 0))
        );
        assert_eq!(
            s.next_after(at(2026, 9, 7, 9, 0)),
            Some(at(2026, 9, 7, 13, 0))
        );
        assert_eq!(
            s.next_after(at(2026, 9, 7, 13, 0)),
            Some(at(2026, 9, 7, 17, 0))
        );
        // 21 would be next but is outside the range, so it rolls to tomorrow.
        assert_eq!(
            s.next_after(at(2026, 9, 7, 17, 0)),
            Some(at(2026, 9, 8, 9, 0))
        );
    }

    #[test]
    fn weekdays_are_matched_with_sunday_as_zero() {
        // 2026-09-07 is a Monday.
        let s = Schedule::parse("30 3 * * 0").unwrap();
        assert_eq!(
            s.next_after(at(2026, 9, 7, 0, 0)),
            Some(at(2026, 9, 13, 3, 30))
        );
        // 7 is Sunday too.
        let s7 = Schedule::parse("30 3 * * 7").unwrap();
        assert_eq!(
            s7.next_after(at(2026, 9, 7, 0, 0)),
            s.next_after(at(2026, 9, 7, 0, 0))
        );
    }

    #[test]
    fn day_of_month_and_day_of_week_are_unioned_not_intersected() {
        // Vixie cron: with both fields restricted, either one matching is enough.
        // The 1st of September 2026 is a Tuesday; Fridays that month are 4, 11,
        // 18, 25. Starting from the 2nd, the next hit is Friday the 4th.
        let s = Schedule::parse("0 0 1 * 5").unwrap();
        assert_eq!(
            s.next_after(at(2026, 9, 2, 0, 0)),
            Some(at(2026, 9, 4, 0, 0))
        );
        // And the 1st of October matches on day-of-month alone (it is a Thursday).
        assert_eq!(
            s.next_after(at(2026, 9, 26, 0, 0)),
            Some(at(2026, 10, 1, 0, 0))
        );
    }

    #[test]
    fn only_day_of_month_restricted_means_intersection_is_not_applied() {
        let s = Schedule::parse("0 0 15 * *").unwrap();
        assert_eq!(
            s.next_after(at(2026, 9, 1, 0, 0)),
            Some(at(2026, 9, 15, 0, 0))
        );
    }

    #[test]
    fn a_month_specific_schedule_crosses_the_year() {
        let s = Schedule::parse("0 0 1 1 *").unwrap();
        assert_eq!(
            s.next_after(at(2026, 9, 7, 0, 0)),
            Some(at(2027, 1, 1, 0, 0))
        );
    }

    #[test]
    fn the_29th_of_february_finds_the_next_leap_year() {
        let s = Schedule::parse("0 0 29 2 *").unwrap();
        // 2027 is not a leap year, 2028 is.
        assert_eq!(
            s.next_after(at(2026, 9, 7, 0, 0)),
            Some(at(2028, 2, 29, 0, 0))
        );
    }

    #[test]
    fn an_impossible_date_yields_none_instead_of_looping_forever() {
        let s = Schedule::parse("0 0 30 2 *").unwrap();
        assert_eq!(s.next_after(at(2026, 9, 7, 0, 0)), None);
    }

    #[test]
    fn bad_expressions_are_rejected_with_a_reason() {
        for expr in [
            "* * * *",      // too few fields
            "* * * * * *",  // too many
            "60 * * * *",   // minute out of range
            "* 24 * * *",   // hour out of range
            "* * 0 * *",    // day-of-month starts at 1
            "* * * 13 *",   // month out of range
            "* * * * 8",    // weekday out of range
            "*/0 * * * *",  // zero step
            "5-1 * * * *",  // backwards range
            "a * * * *",    // not a number
            "1,,2 * * * *", // empty list entry
        ] {
            assert!(
                Schedule::parse(expr).is_err(),
                "{expr:?} should have been rejected"
            );
        }
    }

    #[test]
    fn errors_name_the_offending_field() {
        let err = Schedule::parse("* 24 * * *").unwrap_err().to_string();
        assert!(err.contains("hour"), "{err}");
        assert!(err.contains("24"), "{err}");
    }

    #[test]
    fn schedules_deserialize_from_a_plain_string() {
        #[derive(serde::Deserialize)]
        struct Holder {
            schedule: Schedule,
        }
        let h: Holder = toml::from_str(r#"schedule = "0 3 * * *""#).unwrap();
        assert_eq!(h.schedule.as_str(), "0 3 * * *");

        assert!(toml::from_str::<Holder>(r#"schedule = "nope""#).is_err());
    }
}
