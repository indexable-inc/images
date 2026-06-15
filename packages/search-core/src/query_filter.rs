//! Building a server-side metadata [`Filter`] from a query's scope selectors.
//!
//! One builder, shared by the CLI, the Python binding, and the MCP tool, so the
//! mapping from "the user asked for these sources, this repo" to the wire filter
//! lives in exactly one place.

use mixedbread::{Filter, Operator};
use snafu::Snafu;
use source_meta::{Source, keys};

/// The scope selectors a caller can apply, before they become a [`Filter`].
#[derive(Debug, Default, Clone)]
pub struct FilterSpec {
    /// Restrict to these sources. Empty means all sources.
    pub sources: Vec<Source>,
    /// Exclude these sources.
    pub exclude_sources: Vec<Source>,
    /// Restrict code to this repository slug.
    pub repo: Option<String>,
    /// Restrict to records authored by these users. Empty means all users; one
    /// value (e.g. the current `$USER`) is "only my messages".
    pub users: Vec<String>,
    /// Restrict to records recorded on these hosts. Empty means all hosts.
    pub hosts: Vec<String>,
    /// Restrict to these project slugs (the per-source project tag, e.g. the
    /// directory a Claude transcript was recorded under). Empty means all.
    pub projects: Vec<String>,
    /// Keep only records whose [`keys::TIMESTAMP`] (epoch seconds) is at or
    /// after this instant. `None` means no lower bound.
    pub since: Option<i64>,
    /// Keep only records whose [`keys::TIMESTAMP`] (epoch seconds) is at or
    /// before this instant. `None` means no upper bound.
    pub until: Option<i64>,
}

impl FilterSpec {
    /// Whether any selector is set.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.sources.is_empty()
            && self.exclude_sources.is_empty()
            && self.repo.is_none()
            && self.users.is_empty()
            && self.hosts.is_empty()
            && self.projects.is_empty()
            && self.since.is_none()
            && self.until.is_none()
    }
}

/// A time argument that could not be parsed as epoch seconds or a relative
/// span.
#[derive(Debug, Snafu)]
#[snafu(display(
    "invalid time {value:?}: pass epoch seconds (e.g. 1781200000) or a relative span like 30m, 24h, 7d, 2w"
))]
pub struct InvalidTimeSpec {
    /// The rejected input.
    value: String,
}

/// Parse a user-facing time argument into epoch seconds.
///
/// Accepts a bare integer (epoch seconds, passed through) or a relative span
/// `<n><unit>` with unit `s`/`m`/`h`/`d`/`w` (e.g. `24h`, `7d`), resolved
/// against `now` (epoch seconds) by subtraction — "7d" means "7 days ago".
/// Shared by the CLI and the Python binding so both edges accept the same
/// grammar.
///
/// # Errors
/// Returns [`InvalidTimeSpec`] for anything else (including negative or empty
/// values).
pub fn parse_time_spec(value: &str, now: i64) -> Result<i64, InvalidTimeSpec> {
    let invalid = || InvalidTimeSpec {
        value: value.to_owned(),
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(invalid());
    }
    if trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return trimmed.parse::<i64>().map_err(|_| invalid());
    }
    let (count, unit) = trimmed.split_at(trimmed.len() - 1);
    let count: i64 = count.parse().map_err(|_| invalid())?;
    if count < 0 {
        return Err(invalid());
    }
    let unit_seconds = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        "w" => 7 * 86_400,
        _ => return Err(invalid()),
    };
    let span = count.checked_mul(unit_seconds).ok_or_else(invalid)?;
    Ok(now.saturating_sub(span))
}

/// Build the metadata filter for a [`FilterSpec`], or `None` when nothing is
/// constrained (search all sources).
#[must_use]
pub fn build_filter(spec: &FilterSpec) -> Option<Filter> {
    let mut clauses = Vec::new();

    if !spec.sources.is_empty() {
        let values: Vec<serde_json::Value> =
            spec.sources.iter().map(|s| s.as_str().into()).collect();
        clauses.push(Filter::any_of(
            keys::SOURCE,
            serde_json::Value::Array(values),
        ));
    }
    if !spec.exclude_sources.is_empty() {
        let excluded: Vec<Filter> = spec
            .exclude_sources
            .iter()
            .map(|s| Filter::eq(keys::SOURCE, s.as_str()))
            .collect();
        clauses.push(Filter::none(excluded));
    }
    if let Some(repo) = &spec.repo {
        clauses.push(Filter::eq(keys::REPO, repo.clone()));
    }
    clauses.extend(any_of_strings(keys::USER, &spec.users));
    clauses.extend(any_of_strings(keys::HOST, &spec.hosts));
    clauses.extend(any_of_strings(keys::PROJECT, &spec.projects));
    // Inclusive bounds over the epoch-second TIMESTAMP every adapter writes.
    if let Some(since) = spec.since {
        clauses.push(Filter::condition(keys::TIMESTAMP, Operator::Gte, since));
    }
    if let Some(until) = spec.until {
        clauses.push(Filter::condition(keys::TIMESTAMP, Operator::Lte, until));
    }

    match clauses.len() {
        0 => None,
        1 => clauses.pop(),
        _ => Some(Filter::all(clauses)),
    }
}

/// An `in` filter over `key` for a set of string values, or `None` when the set
/// is empty (no constraint on that key).
fn any_of_strings(key: &str, values: &[String]) -> Option<Filter> {
    if values.is_empty() {
        return None;
    }
    let array: Vec<serde_json::Value> = values.iter().map(|v| v.as_str().into()).collect();
    Some(Filter::any_of(key, serde_json::Value::Array(array)))
}

#[cfg(test)]
mod tests {
    use super::{FilterSpec, build_filter, parse_time_spec};
    use source_meta::Source;

    #[test]
    fn empty_spec_builds_no_filter() {
        assert!(build_filter(&FilterSpec::default()).is_none());
    }

    #[test]
    fn since_until_pin_the_timestamp_wire_shape() {
        // The server-side recency window: inclusive gte/lte conditions on the
        // epoch-second `timestamp` key every adapter writes.
        let spec = FilterSpec {
            since: Some(1_780_000_000),
            until: Some(1_781_000_000),
            ..FilterSpec::default()
        };
        let filter = build_filter(&spec).expect("filter");
        let value = serde_json::to_value(&filter).expect("ser");
        assert_eq!(
            value,
            serde_json::json!({ "all": [
                { "key": "timestamp", "operator": "gte", "value": 1_780_000_000_i64 },
                { "key": "timestamp", "operator": "lte", "value": 1_781_000_000_i64 }
            ] })
        );
    }

    #[test]
    fn since_combines_with_source_under_all() {
        let spec = FilterSpec {
            sources: vec![Source::new("shell")],
            since: Some(1_780_000_000),
            ..FilterSpec::default()
        };
        let filter = build_filter(&spec).expect("filter");
        let value = serde_json::to_value(&filter).expect("ser");
        assert_eq!(
            value,
            serde_json::json!({ "all": [
                { "key": "source", "operator": "in", "value": ["shell"] },
                { "key": "timestamp", "operator": "gte", "value": 1_780_000_000_i64 }
            ] })
        );
    }

    #[test]
    fn time_spec_accepts_epoch_and_relative_spans() {
        let now = 1_781_300_000;
        // Bare digits are epoch seconds, passed through untouched.
        assert_eq!(parse_time_spec("1781200000", now).expect("epoch"), 1_781_200_000);
        // Relative spans subtract from `now`.
        assert_eq!(parse_time_spec("90s", now).expect("s"), now - 90);
        assert_eq!(parse_time_spec("30m", now).expect("m"), now - 30 * 60);
        assert_eq!(parse_time_spec("24h", now).expect("h"), now - 24 * 3600);
        assert_eq!(parse_time_spec("7d", now).expect("d"), now - 7 * 86_400);
        assert_eq!(parse_time_spec("2w", now).expect("w"), now - 14 * 86_400);
        assert_eq!(parse_time_spec(" 24h ", now).expect("trimmed"), now - 24 * 3600);
    }

    #[test]
    fn time_spec_rejects_garbage() {
        let now = 1_781_300_000;
        for bad in ["", "yesterday", "7x", "-5d", "d", "1.5h", "24h7d"] {
            assert!(parse_time_spec(bad, now).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn single_source_builds_an_in_filter() {
        let spec = FilterSpec {
            sources: vec![Source::new("slack")],
            ..FilterSpec::default()
        };
        let filter = build_filter(&spec).expect("filter");
        let value = serde_json::to_value(&filter).expect("ser");
        assert_eq!(
            value,
            serde_json::json!({ "key": "source", "operator": "in", "value": ["slack"] })
        );
    }

    #[test]
    fn exclude_builds_a_none_filter() {
        let spec = FilterSpec {
            exclude_sources: vec![Source::new("slack")],
            ..FilterSpec::default()
        };
        let filter = build_filter(&spec).expect("filter");
        let value = serde_json::to_value(&filter).expect("ser");
        assert_eq!(
            value,
            serde_json::json!({ "none": [ { "key": "source", "operator": "eq", "value": "slack" } ] })
        );
    }

    #[test]
    fn single_user_builds_an_in_filter() {
        let spec = FilterSpec {
            users: vec!["andrew".to_owned()],
            ..FilterSpec::default()
        };
        let filter = build_filter(&spec).expect("filter");
        let value = serde_json::to_value(&filter).expect("ser");
        assert_eq!(
            value,
            serde_json::json!({ "key": "user", "operator": "in", "value": ["andrew"] })
        );
    }

    #[test]
    fn user_host_project_combine_under_all() {
        let spec = FilterSpec {
            users: vec!["andrew".to_owned()],
            hosts: vec!["hydra".to_owned()],
            projects: vec!["ix".to_owned(), "index".to_owned()],
            ..FilterSpec::default()
        };
        let filter = build_filter(&spec).expect("filter");
        let value = serde_json::to_value(&filter).expect("ser");
        assert_eq!(
            value,
            serde_json::json!({ "all": [
                { "key": "user", "operator": "in", "value": ["andrew"] },
                { "key": "host", "operator": "in", "value": ["hydra"] },
                { "key": "project", "operator": "in", "value": ["ix", "index"] }
            ] })
        );
    }

    #[test]
    fn source_and_repo_combine_under_all() {
        let spec = FilterSpec {
            sources: vec![Source::code()],
            repo: Some("indexable-inc/index".to_owned()),
            ..FilterSpec::default()
        };
        let filter = build_filter(&spec).expect("filter");
        let value = serde_json::to_value(&filter).expect("ser");
        assert_eq!(
            value,
            serde_json::json!({ "all": [
                { "key": "source", "operator": "in", "value": ["code"] },
                { "key": "repo", "operator": "eq", "value": "indexable-inc/index" }
            ] })
        );
    }
}
