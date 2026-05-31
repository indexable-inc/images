//! Building a server-side metadata [`Filter`] from a query's scope selectors.
//!
//! One builder, shared by the CLI, the Python binding, and the MCP tool, so the
//! mapping from "the user asked for these sources, this repo" to the wire filter
//! lives in exactly one place.

use mixedbread::Filter;
use search_meta::{Source, keys};

/// The scope selectors a caller can apply, before they become a [`Filter`].
#[derive(Debug, Default, Clone)]
pub struct FilterSpec {
    /// Restrict to these sources. Empty means all sources.
    pub sources: Vec<Source>,
    /// Exclude these sources.
    pub exclude_sources: Vec<Source>,
    /// Restrict code to this repository slug.
    pub repo: Option<String>,
}

impl FilterSpec {
    /// Whether any selector is set.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.sources.is_empty() && self.exclude_sources.is_empty() && self.repo.is_none()
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

    match clauses.len() {
        0 => None,
        1 => clauses.pop(),
        _ => Some(Filter::all(clauses)),
    }
}

#[cfg(test)]
mod tests {
    use super::{FilterSpec, build_filter};
    use search_meta::Source;

    #[test]
    fn empty_spec_builds_no_filter() {
        assert!(build_filter(&FilterSpec::default()).is_none());
    }

    #[test]
    fn single_source_builds_an_in_filter() {
        let spec = FilterSpec {
            sources: vec![Source::Slack],
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
            exclude_sources: vec![Source::Slack],
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
    fn source_and_repo_combine_under_all() {
        let spec = FilterSpec {
            sources: vec![Source::Code],
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
