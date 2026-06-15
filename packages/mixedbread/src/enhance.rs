//! Query enhancement: `/v1/stores/queries/enhance`.
//!
//! The endpoint reads a natural-language query against one or more stores and
//! extracts the structured intent the model can see in it: metadata filter
//! conditions (matched against the stores' real metadata facets), and — when
//! the query asks for a ranking rather than a semantic match ("newest shell
//! commands") — a metadata sort. The response carries exactly one item, either
//! a [`EnhancedQuery::Query`] (semantic search with derived filters) or a
//! [`EnhancedQuery::Sort`] (deterministic metadata ranking, no semantic
//! query), discriminated by its `type` field.

use serde::Deserialize;

use crate::filter::{Condition, Filter};

/// How an enhance item's extracted conditions combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterMode {
    /// Every condition must match (AND) — the API default.
    #[default]
    All,
    /// At least one condition must match (OR).
    Any,
}

/// Ranking direction for an enhance `sort` item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    /// Ascending (oldest/smallest first).
    Asc,
    /// Descending (newest/largest first).
    Desc,
}

impl SortDirection {
    /// Whether this direction is ascending, the form [`crate::SortBy`] wants.
    #[must_use]
    pub const fn is_ascending(self) -> bool {
        matches!(self, Self::Asc)
    }
}

/// One enhanced query item, the payload of the response's single-element
/// `items` list.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EnhancedQuery {
    /// A semantic search: possibly rewritten query text plus extracted
    /// metadata filter conditions.
    Query {
        /// Query text to search with (may equal the input).
        query: String,
        /// Extracted filter conditions; `None`/empty when the model found no
        /// metadata constraints in the query.
        #[serde(default)]
        metadata_filters: Option<Vec<Condition>>,
        /// How `metadata_filters` combine.
        #[serde(default)]
        filter_mode: FilterMode,
    },
    /// A metadata ranking ("newest X"): no semantic query at all; rank by a
    /// metadata field instead, optionally under extracted filters.
    Sort {
        /// Extracted filter conditions; combination as in the query variant.
        #[serde(default)]
        metadata_filters: Option<Vec<Condition>>,
        /// How `metadata_filters` combine.
        #[serde(default)]
        filter_mode: FilterMode,
        /// Metadata field to rank by (e.g. `timestamp`).
        rank_by: String,
        /// Ranking direction.
        direction: SortDirection,
    },
}

impl EnhancedQuery {
    /// The extracted conditions as one [`Filter`] ready to pass to search,
    /// grep, or chunk listing: a single condition stays a leaf, several are
    /// grouped per the item's [`FilterMode`]. `None` when nothing was
    /// extracted.
    #[must_use]
    pub fn filter(&self) -> Option<Filter> {
        let (conditions, mode) = match self {
            Self::Query {
                metadata_filters,
                filter_mode,
                ..
            }
            | Self::Sort {
                metadata_filters,
                filter_mode,
                ..
            } => (metadata_filters.as_ref()?, *filter_mode),
        };
        let mut filters: Vec<Filter> = conditions
            .iter()
            .cloned()
            .map(Filter::Condition)
            .collect();
        match filters.len() {
            0 => None,
            1 => filters.pop(),
            _ => Some(match mode {
                FilterMode::All => Filter::all(filters),
                FilterMode::Any => Filter::any(filters),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::filter::Operator;

    use super::{EnhancedQuery, FilterMode, SortDirection};

    #[test]
    fn a_query_item_deserializes_with_filters_and_mode() {
        // The exact body the live API returned for "slack messages from andrew
        // in the last week about the indexer" (2026-06-12); the `type` tag must
        // select the query variant and the null rank fields must not break it.
        let wire = serde_json::json!({
            "type": "query",
            "query": "slack messages about the indexer",
            "metadata_filters": [
                { "key": "user", "operator": "eq", "value": "andrew" },
                { "key": "timestamp", "operator": "gte", "value": 1_780_617_600 }
            ],
            "filter_mode": "all",
            "rank_by": null,
            "direction": null
        });
        let item: EnhancedQuery = serde_json::from_value(wire).expect("deserialize");
        let EnhancedQuery::Query {
            query,
            metadata_filters,
            filter_mode,
        } = &item
        else {
            panic!("expected query item, got {item:?}");
        };
        assert_eq!(query, "slack messages about the indexer");
        assert_eq!(*filter_mode, FilterMode::All);
        let conditions = metadata_filters.as_ref().expect("filters");
        assert_eq!(conditions.len(), 2);
        assert_eq!(conditions[0].key, "user");
        assert_eq!(conditions[1].operator, Operator::Gte);

        // The derived filter groups both conditions under `all`.
        let filter = item.filter().expect("filter");
        assert_eq!(
            serde_json::to_value(&filter).expect("serialize"),
            serde_json::json!({ "all": [
                { "key": "user", "operator": "eq", "value": "andrew" },
                { "key": "timestamp", "operator": "gte", "value": 1_780_617_600 }
            ] })
        );
    }

    #[test]
    fn a_sort_item_deserializes_and_a_single_condition_stays_a_leaf() {
        // Live shape for "newest shell commands" (2026-06-12): a sort item with
        // null filters. Add one condition to pin the leaf (ungrouped) form.
        let wire = serde_json::json!({
            "type": "sort",
            "metadata_filters": [
                { "key": "source", "operator": "eq", "value": "shell" }
            ],
            "filter_mode": "all",
            "rank_by": "timestamp",
            "direction": "desc"
        });
        let item: EnhancedQuery = serde_json::from_value(wire).expect("deserialize");
        let EnhancedQuery::Sort {
            rank_by, direction, ..
        } = &item
        else {
            panic!("expected sort item, got {item:?}");
        };
        assert_eq!(rank_by, "timestamp");
        assert_eq!(*direction, SortDirection::Desc);
        assert!(!direction.is_ascending());

        assert_eq!(
            serde_json::to_value(item.filter().expect("filter")).expect("serialize"),
            serde_json::json!({ "key": "source", "operator": "eq", "value": "shell" })
        );
    }

    #[test]
    fn empty_or_null_filters_derive_no_filter() {
        let null_filters: EnhancedQuery = serde_json::from_value(serde_json::json!({
            "type": "sort",
            "metadata_filters": null,
            "rank_by": "timestamp",
            "direction": "asc"
        }))
        .expect("deserialize");
        assert!(null_filters.filter().is_none());

        let empty: EnhancedQuery = serde_json::from_value(serde_json::json!({
            "type": "query",
            "query": "q",
            "metadata_filters": []
        }))
        .expect("deserialize");
        assert!(empty.filter().is_none());
    }
}
