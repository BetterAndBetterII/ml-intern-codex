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

