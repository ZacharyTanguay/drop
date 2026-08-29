//! ZOUGCLOUD(ZC-008): display strings for playtime.
//!
//! Formatted in Rust rather than in the Vue layer so the rules are unit-tested
//! and the frontend just renders a string. English only, matching the rest of
//! Drop Desktop, which has no i18n.

use chrono::{DateTime, Utc};

/// Above this, minutes stop earning their place on the line.
const HOURS_ONLY_THRESHOLD_SECONDS: u64 = 100 * 3600;

/// Human playtime, or `None` when the game has never been played.
///
/// `None` rather than "0 min" so the caller can simply omit the whole element
/// instead of rendering a zero next to "Up to date".
pub fn format_playtime(total_seconds: u64) -> Option<String> {
    if total_seconds == 0 {
        return None;
    }

    if total_seconds < 60 {
        return Some("Less than a minute played".to_owned());
    }

    if total_seconds < 3600 {
        return Some(format!("{} min played", total_seconds / 60));
    }

    let hours = total_seconds / 3600;

    // Past a hundred hours the minutes are noise, and the line reads better
    // without them.
    if total_seconds >= HOURS_ONLY_THRESHOLD_SECONDS {
        return Some(format!("{hours}h played"));
    }

    let minutes = (total_seconds % 3600) / 60;
    Some(format!("{hours}h {minutes}m played"))
}

/// Secondary "Last played …" text, or `None` if never played.
///
/// Deliberately coarse: this is a tooltip/detail line, not a timestamp.
pub fn format_last_played(last_played_at: Option<i64>, now: i64) -> Option<String> {
    let last = last_played_at?;
    let last_dt = DateTime::<Utc>::from_timestamp(last, 0)?;
    let now_dt = DateTime::<Utc>::from_timestamp(now, 0)?;

    let days = (now_dt.date_naive() - last_dt.date_naive()).num_days();

    let when = match days {
        d if d <= 0 => "today".to_owned(),
        1 => "yesterday".to_owned(),
        2..=6 => format!("{days} days ago"),
        _ => last_dt.format("%b %-d").to_string(),
    };

    Some(format!("Last played {when}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_played_shows_nothing() {
        assert_eq!(format_playtime(0), None);
    }

    #[test]
    fn under_a_minute() {
        assert_eq!(
            format_playtime(1).as_deref(),
            Some("Less than a minute played")
        );
        assert_eq!(
            format_playtime(59).as_deref(),
            Some("Less than a minute played")
        );
    }

    #[test]
    fn minutes() {
        assert_eq!(format_playtime(60).as_deref(), Some("1 min played"));
        assert_eq!(format_playtime(42 * 60).as_deref(), Some("42 min played"));
        assert_eq!(format_playtime(3599).as_deref(), Some("59 min played"));
    }

    #[test]
    fn hours_and_minutes() {
        assert_eq!(format_playtime(3600).as_deref(), Some("1h 0m played"));
        assert_eq!(
            format_playtime(12 * 3600 + 34 * 60).as_deref(),
            Some("12h 34m played")
        );
        assert_eq!(
            format_playtime(99 * 3600 + 59 * 60).as_deref(),
            Some("99h 59m played")
        );
    }

    #[test]
    fn large_totals_drop_the_minutes() {
        assert_eq!(format_playtime(100 * 3600).as_deref(), Some("100h played"));
        assert_eq!(
            format_playtime(124 * 3600 + 37 * 60).as_deref(),
            Some("124h played")
        );
    }

    const DAY: i64 = 86_400;
    // 2026-08-29T12:00:00Z
    const NOW: i64 = 1_787_918_400;

    #[test]
    fn last_played_is_relative_when_recent() {
        assert_eq!(
            format_last_played(Some(NOW - 3600), NOW).as_deref(),
            Some("Last played today")
        );
        assert_eq!(
            format_last_played(Some(NOW - DAY), NOW).as_deref(),
            Some("Last played yesterday")
        );
        assert_eq!(
            format_last_played(Some(NOW - 3 * DAY), NOW).as_deref(),
            Some("Last played 3 days ago")
        );
    }

    #[test]
    fn last_played_falls_back_to_a_date() {
        let text = format_last_played(Some(NOW - 40 * DAY), NOW).expect("some");
        assert!(text.starts_with("Last played "), "{text}");
        assert!(!text.contains("days ago"), "{text}");
    }

    #[test]
    fn never_played_has_no_last_played() {
        assert_eq!(format_last_played(None, NOW), None);
    }
}
