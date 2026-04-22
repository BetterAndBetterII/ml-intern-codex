use std::time::SystemTime;

pub fn human_time_ago(ts: SystemTime, now: SystemTime) -> String {
    let secs = now.duration_since(ts).unwrap_or_default().as_secs();
    let format_unit = |value: u64, unit: &str| {
        if value == 1 {
            format!("{value} {unit} ago")
        } else {
            format!("{value} {unit}s ago")
        }
    };

    if secs < 60 {
        return format_unit(secs, "second");
    }

    if secs < 60 * 60 {
        return format_unit(secs / 60, "minute");
    }

    if secs < 60 * 60 * 24 {
        return format_unit(secs / 3600, "hour");
    }

    format_unit(secs / (60 * 60 * 24), "day")
}

#[cfg(test)]
mod tests {
    use super::human_time_ago;
    use pretty_assertions::assert_eq;
    use std::time::Duration;
    use std::time::UNIX_EPOCH;

    #[test]
    fn human_time_ago_formats_seconds_minutes_hours_and_days() {
        let now = UNIX_EPOCH + Duration::from_secs(10_000);
        assert_eq!(
            human_time_ago(now - Duration::from_secs(1), now),
            "1 second ago"
        );
        assert_eq!(
            human_time_ago(now - Duration::from_secs(2), now),
            "2 seconds ago"
        );
        assert_eq!(
            human_time_ago(now - Duration::from_secs(60), now),
            "1 minute ago"
        );
        assert_eq!(
            human_time_ago(now - Duration::from_secs(60 * 2), now),
            "2 minutes ago"
        );
        assert_eq!(
            human_time_ago(now - Duration::from_secs(60 * 60), now),
            "1 hour ago"
        );
        assert_eq!(
            human_time_ago(now - Duration::from_secs(60 * 60 * 2), now),
            "2 hours ago"
        );
        assert_eq!(
            human_time_ago(now - Duration::from_secs(60 * 60 * 24), now),
            "1 day ago"
        );
        assert_eq!(
            human_time_ago(now - Duration::from_secs(60 * 60 * 24 * 2), now),
            "2 days ago"
        );
    }

    #[test]
    fn human_time_ago_handles_future_timestamps_as_zero() {
        let now = UNIX_EPOCH + Duration::from_secs(10);
        assert_eq!(
            human_time_ago(now + Duration::from_secs(1), now),
            "0 seconds ago"
        );
    }
}
