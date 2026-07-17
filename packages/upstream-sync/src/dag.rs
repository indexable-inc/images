//! Reader for each series' `dag.json`, the machine-derived dependency graph
//! that `rebase-patches` regenerates and the `patch-dag-<name>` check
//! staleness-gates.
//!
//! This crate only ever READS it: node order is the canonical series, and
//! the ancestor closure decides what an upstream contribution must drag
//! along.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use color_eyre::eyre::{Result, WrapErr, eyre};
use serde::Deserialize;

/// The dag.json document (extra fields like `comment` are other tools' data).
#[derive(Debug, Deserialize)]
pub struct Doc {
    pub nodes: Vec<Node>,
}

/// One patch node with its direct in-series dependencies.
#[derive(Debug, Deserialize)]
pub struct Node {
    pub patch: String,
    #[serde(default)]
    pub deps: Vec<String>,
}

impl Doc {
    /// Load a series' dag.json.
    ///
    /// # Errors
    /// Fails when the file is unreadable or not the expected shape.
    pub fn load(path: &Path) -> Result<Self> {
        let raw =
            fs::read_to_string(path).wrap_err_with(|| format!("cannot read {}", path.display()))?;
        serde_json::from_str(&raw).wrap_err_with(|| format!("cannot parse {}", path.display()))
    }

    /// Patch names in node order (the canonical series).
    #[must_use]
    pub fn patch_names(&self) -> Vec<String> {
        self.nodes.iter().map(|n| n.patch.clone()).collect()
    }

    /// Direct-dependency map keyed by patch name.
    #[must_use]
    pub fn deps_of(&self) -> BTreeMap<String, Vec<String>> {
        self.nodes
            .iter()
            .map(|n| (n.patch.clone(), n.deps.clone()))
            .collect()
    }

    /// The transitive ancestor closure of `patch` (excluding the patch
    /// itself), in discovery order.
    #[must_use]
    pub fn closure(&self, patch: &str) -> Vec<String> {
        let deps_of = self.deps_of();
        let mut seen: Vec<String> = Vec::new();
        let mut stack: Vec<String> = deps_of.get(patch).cloned().unwrap_or_default();
        while let Some(cur) = stack.first().cloned() {
            stack.remove(0);
            if seen.contains(&cur) {
                continue;
            }
            seen.push(cur.clone());
            if let Some(more) = deps_of.get(&cur) {
                stack.extend(more.iter().cloned());
            }
        }
        seen
    }
}

/// Resolve a user-provided patch reference to an exact node name: exact
/// match, else unique prefix, else unique substring.
///
/// # Errors
/// Fails when nothing matches, or when the reference is ambiguous.
pub fn resolve(reference: &str, names: &[String]) -> Result<String> {
    if names.iter().any(|n| n == reference) {
        return Ok(reference.to_owned());
    }
    let by_prefix: Vec<&String> = names.iter().filter(|n| n.starts_with(reference)).collect();
    if let [only] = by_prefix.as_slice() {
        return Ok((*only).clone());
    }
    let by_sub: Vec<&String> = names.iter().filter(|n| n.contains(reference)).collect();
    if let [only] = by_sub.as_slice() {
        return Ok((*only).clone());
    }
    let mut candidates: Vec<&String> = by_prefix;
    for c in by_sub {
        if !candidates.contains(&c) {
            candidates.push(c);
        }
    }
    if candidates.is_empty() {
        return Err(eyre!(
            "upstream-pr: no patch matching '{reference}'. Known: {}",
            names.join(", ")
        ));
    }
    Err(eyre!(
        "upstream-pr: '{reference}' is ambiguous; matches: {}",
        candidates
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Doc {
        serde_json::from_str(
            r#"{"nodes":[
                {"patch":"0001-a.patch","deps":[]},
                {"patch":"0002-b.patch","deps":["0001-a.patch"]},
                {"patch":"0003-c.patch","deps":["0002-b.patch"]}
            ]}"#,
        )
        .unwrap()
    }

    #[test]
    fn closure_is_transitive_and_excludes_target() {
        let d = doc();
        assert!(d.closure("0001-a.patch").is_empty());
        assert_eq!(
            d.closure("0003-c.patch"),
            vec!["0002-b.patch".to_owned(), "0001-a.patch".to_owned()]
        );
    }

    #[test]
    fn resolve_exact_prefix_substring_ambiguous() {
        let names = doc().patch_names();
        assert_eq!(resolve("0002-b.patch", &names).unwrap(), "0002-b.patch");
        assert_eq!(resolve("0002", &names).unwrap(), "0002-b.patch");
        assert_eq!(resolve("-c.", &names).unwrap(), "0003-c.patch");
        assert!(resolve("000", &names).is_err());
        assert!(resolve("zzz", &names).is_err());
    }
}
