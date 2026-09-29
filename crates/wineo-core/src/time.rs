//! Formatting times and durations the same way in every frontend.

use jiff::Timestamp;
use jiff::tz::TimeZone;

/// Local wall-clock time, `%Y-%m-%d %H:%M:%S`.
pub fn absolute(ts: Timestamp, tz: &TimeZone) -> String {
    ts.to_zoned(tz.clone())
        .strftime("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// `HH:MM` if `ts` falls on today in `tz`, else `YYYY-MM-DD HH:MM`.
pub fn clock(now: Timestamp, ts: Timestamp, tz: &TimeZone) -> String {
    let zoned = ts.to_zoned(tz.clone());
    if zoned.date() == now.to_zoned(tz.clone()).date() {
        zoned.strftime("%H:%M").to_string()
    } else {
        zoned.strftime("%Y-%m-%d %H:%M").to_string()
    }
}

/// A duration in hours and minutes, like `wineo stats` shows: `107 h 19 m`,
/// `13 m` under an hour, or `95 h` when the minutes are exactly zero.
pub fn duration_hm(seconds: i64) -> String {
    let minutes = seconds.max(0) / 60;
    let (hours, minutes) = (minutes / 60, minutes % 60);
    if hours == 0 {
        format!("{minutes} m")
    } else if minutes == 0 {
        format!("{hours} h")
    } else {
        format!("{hours} h {minutes} m")
    }
}

/// `today`, `yesterday`, or a bare date like `Sep 4`, for when something last
/// happened, in `tz`.
pub fn day_label(now: Timestamp, ts: Timestamp, tz: &TimeZone) -> String {
    let today = now.to_zoned(tz.clone()).date();
    let date = ts.to_zoned(tz.clone()).date();
    if date == today {
        "today".to_owned()
    } else if date == today.yesterday().unwrap_or(today) {
        "yesterday".to_owned()
    } else {
        date.strftime("%b %-d").to_string()
    }
}

/// How long ago `then` was, rounded down. Clock skew that puts `then` in the
/// future is clamped to "0 s ago" instead of going negative.
pub fn relative(now: Timestamp, then: Timestamp) -> String {
    let secs = now.duration_since(then).as_secs().max(0);
    if secs < 60 {
        format!("{secs} s ago")
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else if secs < 48 * 3600 {
        format!("{} h ago", secs / 3600)
    } else {
        format!("{} days ago", secs / 86400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn relative_rounds_down_through_each_bucket() {
        let now = ts("2026-09-16T14:05:00Z");
        assert_eq!(relative(now, ts("2026-09-16T14:04:31Z")), "29 s ago");
        assert_eq!(relative(now, ts("2026-09-16T14:02:00Z")), "3 min ago");
        assert_eq!(relative(now, ts("2026-09-16T14:04:01Z")), "59 s ago");
        assert_eq!(relative(now, ts("2026-09-16T14:04:00Z")), "1 min ago");
        assert_eq!(relative(now, ts("2026-09-16T13:05:00Z")), "1 h ago");
        assert_eq!(relative(now, ts("2026-09-14T15:05:00Z")), "47 h ago");
        assert_eq!(relative(now, ts("2026-09-14T14:05:00Z")), "2 days ago");
        assert_eq!(relative(now, ts("2026-09-01T14:05:00Z")), "15 days ago");
    }

    #[test]
    fn relative_clamps_future_times_to_zero() {
        let now = ts("2026-09-16T14:05:00Z");
        assert_eq!(relative(now, ts("2026-09-16T14:05:05Z")), "0 s ago");
    }

    #[test]
    fn clock_switches_to_a_full_date_before_today() {
        let now = ts("2026-09-16T14:05:00Z");
        assert_eq!(
            clock(now, ts("2026-09-16T09:02:00Z"), &TimeZone::UTC),
            "09:02"
        );
        assert_eq!(
            clock(now, ts("2026-09-15T23:59:00Z"), &TimeZone::UTC),
            "2026-09-15 23:59"
        );
    }

    #[test]
    fn duration_hm_drops_zero_hours_or_minutes() {
        assert_eq!(duration_hm(107 * 3600 + 19 * 60), "107 h 19 m");
        assert_eq!(duration_hm(13 * 60), "13 m");
        assert_eq!(duration_hm(95 * 3600), "95 h");
        assert_eq!(duration_hm(0), "0 m");
    }

    #[test]
    fn day_label_covers_today_yesterday_and_a_bare_date() {
        let now = ts("2026-09-26T20:00:00Z");
        assert_eq!(
            day_label(now, ts("2026-09-26T09:00:00Z"), &TimeZone::UTC),
            "today"
        );
        assert_eq!(
            day_label(now, ts("2026-09-25T23:00:00Z"), &TimeZone::UTC),
            "yesterday"
        );
        assert_eq!(
            day_label(now, ts("2026-09-04T09:00:00Z"), &TimeZone::UTC),
            "Sep 4"
        );
    }

    #[test]
    fn absolute_formats_local_date_and_time() {
        assert_eq!(
            absolute(ts("2026-09-16T14:02:11Z"), &TimeZone::UTC),
            "2026-09-16 14:02:11"
        );
    }
}
