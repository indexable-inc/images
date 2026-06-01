//! Building a server-side metadata [`Filter`] from a query's scope selectors.
//!
//! One builder, shared by the CLI, the Python binding, and the MCP tool, so the
//! mapping from "the user asked for these sources, this repo" to the wire filter
//! lives in exactly one place.

use mixedbread::Filter;
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
    }
}

/// Build the metadata filter for a [`FilterSpec`], or `None` when nothing is
/// constrained (search all sources).
#[must_use]
pub fn build_filter(spec: &FilterSpec) -> Option<Filter> {
    let mut clauses = Vec::new();

    if !spec.sources.is_empty() {
        let values: Vec<serde_json::Value> =
            spec.sources.iter().map(|s| s.as_str().into()).collect();
        clauses.push(Filter::any_of(keys::SOURCE, serde_json::Value::Array(values)));
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
    use super::{FilterSpec, build_filter};
    use source_meta::Source;

    #[test]
    fn empty_spec_builds_no_filter() {
        assert!(build_filter(&FilterSpec::default()).is_none());
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
