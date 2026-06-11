//! Format a binary's `--version` line from build metadata a Nix wrapper stamps
//! into the environment, so every tool reports its revision, commit date, and
//! how long ago it was built in one consistent shape.
//!
//! A reproducible build has no wall-clock compile time, so the "when" is the
//! flake's commit time ([`EPOCH_ENV`], `self.lastModified`). "How long ago" is
//! computed at run time against the current clock, which is why it cannot be
//! baked into the binary and rides the wrapper env instead.
//!
//! ```no_run
//! // Hand the interned `&'static str` straight to clap's `Command::version`.
//! let version: &'static str = build_version::version_static(env!("CARGO_PKG_VERSION"));
//! ```

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::DateTime;

/// Env var a Nix wrapper sets to the build's flake revision: the full git SHA on
/// a clean tree, `<sha>-dirty` on a dirty one, or `dev` off a non-git source.
/// Mirrors `ix.rev`.
pub const REV_ENV: &str = "IX_BUILD_REV";

/// Env var a Nix wrapper sets to the build's commit time as unix epoch seconds
/// (`self.lastModified`). Mirrors `ix.revEpoch`.
pub const EPOCH_ENV: &str = "IX_BUILD_EPOCH";

/// Length of the abbreviated revision shown in the stamp.
const SHORT_REV_LEN: usize = 12;

/// `--version` text for `crate_version`, interned so the returned `&'static str`
/// satisfies clap's `Command::version` bound (an owned `String` does not).
///
/// When the wrapper env vars are present the version carries the build stamp:
/// `0.1.0 (7e42ccdb1882, 2026-06-07, 2 days ago)`. Outside the packaged wrapper
/// (a dev `cargo run`) the env vars are unset and the bare crate version is
/// returned. Computed once per process; the first call wins, matching how clap
/// reads the version a single time at startup.
#[must_use]
pub fn version_static(crate_version: &str) -> &'static str {
    static CACHE: OnceLock<String> = OnceLock::new();
    CACHE.get_or_init(|| {
        std::env::var(REV_ENV)
            .ok()
            .filter(|rev| !rev.is_empty())
            .map_or_else(
                || crate_version.to_owned(),
                |rev| {
                    let epoch = std::env::var(EPOCH_ENV)
                        .ok()
                        .and_then(|raw| raw.parse::<i64>().ok());
                    format!(
                        "{crate_version} ({})",
                        stamp(&rev, epoch, SystemTime::now())
                    )
                },
            )
    })
}

/// Build the parenthesised stamp from raw metadata.
///
/// Renders `7e42ccdb1882, 2026-06-07, 2 days ago`. Split out from
/// [`version_static`] so it is testable without touching the environment or the
/// wall clock. The revision is abbreviated; the date and relative age come from
/// `epoch`, and degrade to the short revision alone when no epoch is known.
#[must_use]
pub fn stamp(rev: &str, epoch: Option<i64>, now: SystemTime) -> String {
    let short: String = rev.chars().take(SHORT_REV_LEN).collect();
    // `0` is the `revEpoch ? 0` sentinel for a non-git eval (no real build is
    // dated to 1970), so treat it as "no date" rather than printing an absurd
    // `1970-01-01, 56 years ago`.
    let Some(epoch) = epoch.filter(|&seconds| seconds != 0) else {
        return short;
    };
    let Some(committed) = DateTime::from_timestamp(epoch, 0) else {
        return short;
    };
    let date = committed.format("%Y-%m-%d");
    let now_epoch = now
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(epoch);
    format!("{short}, {date}, {}", humanize_ago(now_epoch - epoch))
}

/// Render an elapsed span in seconds as a short relative phrase.
///
/// For example `just now`, `5 minutes ago`, `2 days ago`, `1 year ago`. Spans
/// under a minute, and negative spans (the build clock is ahead of ours),
/// collapse to `just now` rather than reading `0 minutes ago` or inventing a
/// future.
#[must_use]
pub fn humanize_ago(seconds: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;

    if seconds < MINUTE {
        return "just now".to_owned();
    }

    let (value, unit) = if seconds < HOUR {
        (seconds / MINUTE, "minute")
    } else if seconds < DAY {
        (seconds / HOUR, "hour")
    } else if seconds < WEEK {
        (seconds / DAY, "day")
    } else if seconds < MONTH {
        (seconds / WEEK, "week")
    } else if seconds < YEAR {
        (seconds / MONTH, "month")
    } else {
        (seconds / YEAR, "year")
    };

    let plural = if value == 1 { "" } else { "s" };
    format!("{value} {unit}{plural} ago")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn humanize_ago_buckets_each_unit() {
        assert_eq!(humanize_ago(-5), "just now");
        assert_eq!(humanize_ago(0), "just now");
        assert_eq!(humanize_ago(59), "just now");
        assert_eq!(humanize_ago(60), "1 minute ago");
        assert_eq!(humanize_ago(120), "2 minutes ago");
        assert_eq!(humanize_ago(3600), "1 hour ago");
        assert_eq!(humanize_ago(5 * 3600), "5 hours ago");
        assert_eq!(humanize_ago(2 * 86_400), "2 days ago");
        assert_eq!(humanize_ago(8 * 86_400), "1 week ago");
        assert_eq!(humanize_ago(45 * 86_400), "1 month ago");
        assert_eq!(humanize_ago(400 * 86_400), "1 year ago");
    }

    #[test]
    fn stamp_abbreviates_rev_and_renders_date_and_age() {
        // epoch = 1970-01-02T00:00:00Z (nonzero so it is not the "unknown"
        // sentinel); `now` two days after that.
        let epoch = 86_400;
        // Just over two days after the commit. `from_days` is unstable and a
        // round `from_secs` trips `clippy::duration_suboptimal_units`, so the
        // `+ 1` nudges the value off every whole-unit boundary.
        let now = UNIX_EPOCH + Duration::from_secs(3 * 86_400 + 1);
        let rendered = stamp("7e42ccdb18827401226635", Some(epoch), now);
        assert_eq!(rendered, "7e42ccdb1882, 1970-01-02, 2 days ago");
    }

    #[test]
    fn stamp_without_epoch_is_short_rev_only() {
        let rendered = stamp("7e42ccdb18827401226635", None, SystemTime::now());
        assert_eq!(rendered, "7e42ccdb1882");
    }

    #[test]
    fn stamp_treats_zero_epoch_as_unknown() {
        let rendered = stamp("7e42ccdb18827401226635", Some(0), SystemTime::now());
        assert_eq!(rendered, "7e42ccdb1882");
    }
}
