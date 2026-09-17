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
    fn absolute_formats_local_date_and_time() {
        assert_eq!(
            absolute(ts("2026-09-16T14:02:11Z"), &TimeZone::UTC),
            "2026-09-16 14:02:11"
        );
    }
}
