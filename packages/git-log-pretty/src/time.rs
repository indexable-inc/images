//! Relative timestamp formatting for commit times.

use chrono::{DateTime, Utc};
use color_eyre::eyre::{Result, eyre};

/// Render a Unix timestamp (seconds) as a coarse "N units ago" string relative
/// to now. Granularity steps down from days to hours to minutes, ending at
/// `"just now"` for anything under a minute. Future timestamps (clock skew)
/// also read as `"just now"`.
pub fn relative(timestamp_secs: i64) -> Result<String> {
    let then = DateTime::from_timestamp(timestamp_secs, 0)
        .ok_or_else(|| eyre!("commit timestamp out of range: {timestamp_secs}"))?;
    let elapsed = Utc::now().signed_duration_since(then);

    let label = if elapsed.num_days() > 0 {
        plural(elapsed.num_days(), "day")
    } else if elapsed.num_hours() > 0 {
        plural(elapsed.num_hours(), "hour")
    } else if elapsed.num_minutes() > 0 {
        plural(elapsed.num_minutes(), "minute")
    } else {
        "just now".to_string()
    };

    Ok(label)
}

/// Format `count` with `unit`, appending "ago" and pluralizing the unit.
fn plural(count: i64, unit: &str) -> String {
    let suffix = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{suffix} ago")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_timestamps_read_as_just_now() {
        let now = Utc::now().timestamp();
        assert_eq!(relative(now).unwrap(), "just now");
        // Clock skew should not produce a negative count.
        assert_eq!(relative(now + 5).unwrap(), "just now");
    }

    #[test]
    fn singular_and_plural_units() {
        let now = Utc::now().timestamp();
        assert_eq!(relative(now - 3600).unwrap(), "1 hour ago");
        assert_eq!(relative(now - 2 * 3600).unwrap(), "2 hours ago");
        assert_eq!(relative(now - 86_400).unwrap(), "1 day ago");
    }
}
