//! Metadata filters for store search, grep, question-answering, and file
//! listing.
//!
//! The wire shape mirrors the Mixedbread API's recursive filter: a leaf
//! [`Condition`] (`{key, operator, value}`) or a [`Group`] combining nested
//! filters with `all` (AND), `any` (OR), or `none` (NOT). The same `filters`
//! field is accepted by `/v1/stores/search`, `/v1/stores/grep`,
//! `/v1/stores/question-answering`, and the file-list endpoint, so one type
//! serves them all.
//!
//! This type is serialize-only: the client sends filters but never parses them
//! back. [`Filter`] is `#[serde(untagged)]` because a leaf and a group have
//! disjoint key sets, so a serializer always emits the right shape.

use serde::Serialize;

/// A comparison operator on a metadata key.
///
/// Serializes to the exact lowercase tokens the API expects (`eq`, `not_eq`,
/// `gt`, ...). The variant set matches the SDK's `SearchFilterCondition`
/// operator union, which is the source of truth (the prose docs omit a few).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    /// Equal.
    Eq,
    /// Not equal.
    NotEq,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Gte,
    /// Less than.
    Lt,
    /// Less than or equal.
    Lte,
    /// Member of the given array.
    In,
    /// Not a member of the given array.
    NotIn,
    /// SQL-style `LIKE` match.
    Like,
    /// Negated SQL-style `LIKE` match.
    NotLike,
    /// Value starts with the given prefix.
    StartsWith,
    /// Regular-expression match.
    Regex,
}

/// A single leaf condition: a metadata `key` compared to `value` by `operator`.
#[derive(Debug, Clone, Serialize)]
pub struct Condition {
    /// Metadata key, dot-notated for nested fields (e.g. `generated_metadata.language`).
    pub key: String,
    /// Comparison operator.
    pub operator: Operator,
    /// Right-hand value: a scalar for most operators, an array for `in`/`not_in`.
    pub value: serde_json::Value,
}

/// A logical group combining nested filters. Exactly one combinator is set in
/// practice; all are optional so the wire shape carries only what is used.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Group {
    /// All nested filters must match (AND).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all: Option<Vec<Filter>>,
    /// At least one nested filter must match (OR).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub any: Option<Vec<Filter>>,
    /// No nested filter may match (NOT).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub none: Option<Vec<Filter>>,
}

/// A metadata filter: either a leaf [`Condition`] or a nested [`Group`].
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Filter {
    /// A leaf comparison.
    Condition(Condition),
    /// A logical combination of nested filters.
    Group(Group),
}

impl Filter {
    /// A leaf condition `key <op> value`. `value` is anything serializable
    /// (string, number, bool, or array for `in`/`not_in`).
    #[must_use]
    pub fn condition(key: impl Into<String>, operator: Operator, value: impl Into<serde_json::Value>) -> Self {
        Self::Condition(Condition {
            key: key.into(),
            operator,
            value: value.into(),
        })
    }

    /// Shorthand for `key == value`.
    #[must_use]
    pub fn eq(key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        Self::condition(key, Operator::Eq, value)
    }

    /// Shorthand for `key IN [values]`.
    #[must_use]
    pub fn any_of(key: impl Into<String>, values: impl Into<serde_json::Value>) -> Self {
        Self::condition(key, Operator::In, values)
    }

    /// All of `filters` must match (AND).
    #[must_use]
    pub fn all(filters: Vec<Self>) -> Self {
        Self::Group(Group {
            all: Some(filters),
            ..Group::default()
        })
    }

    /// At least one of `filters` must match (OR).
    #[must_use]
    pub fn any(filters: Vec<Self>) -> Self {
        Self::Group(Group {
            any: Some(filters),
            ..Group::default()
        })
    }

    /// None of `filters` may match (NOT).
    #[must_use]
    pub fn none(filters: Vec<Self>) -> Self {
        Self::Group(Group {
            none: Some(filters),
            ..Group::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Filter, Operator};

    #[test]
    fn operators_serialize_to_exact_lowercase_tokens() {
        let cases = [
            (Operator::Eq, "\"eq\""),
            (Operator::NotEq, "\"not_eq\""),
            (Operator::Gt, "\"gt\""),
            (Operator::Gte, "\"gte\""),
            (Operator::Lt, "\"lt\""),
            (Operator::Lte, "\"lte\""),
            (Operator::In, "\"in\""),
            (Operator::NotIn, "\"not_in\""),
            (Operator::Like, "\"like\""),
            (Operator::NotLike, "\"not_like\""),
            (Operator::StartsWith, "\"starts_with\""),
            (Operator::Regex, "\"regex\""),
        ];
        for (op, expected) in cases {
            assert_eq!(serde_json::to_string(&op).expect("serialize"), expected);
        }
    }

    #[test]
    fn leaf_condition_matches_documented_shape() {
        let filter = Filter::eq("source", "code");
        let value = serde_json::to_value(&filter).expect("serialize");
        assert_eq!(
            value,
            serde_json::json!({ "key": "source", "operator": "eq", "value": "code" })
        );
    }

    #[test]
    fn nested_group_matches_documented_example() {
        // Mirrors the docs example: status published AND (priority>=3 OR (category
        // important AND reviewed true)). Asserts the exact all/any/none nesting.
        let filter = Filter::all(vec![
            Filter::eq("status", "published"),
            Filter::any(vec![
                Filter::condition("priority", Operator::Gte, 3),
                Filter::all(vec![
                    Filter::eq("category", "important"),
                    Filter::eq("reviewed", true),
                ]),
            ]),
        ]);
        let value = serde_json::to_value(&filter).expect("serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "all": [
                    { "key": "status", "operator": "eq", "value": "published" },
                    { "any": [
                        { "key": "priority", "operator": "gte", "value": 3 },
                        { "all": [
                            { "key": "category", "operator": "eq", "value": "important" },
                            { "key": "reviewed", "operator": "eq", "value": true }
                        ] }
                    ] }
                ]
            })
        );
    }

    #[test]
    fn none_excludes_a_source() {
        let filter = Filter::none(vec![Filter::eq("source", "slack")]);
        let value = serde_json::to_value(&filter).expect("serialize");
        assert_eq!(
            value,
            serde_json::json!({ "none": [ { "key": "source", "operator": "eq", "value": "slack" } ] })
        );
    }
}
