//! User supplied excludes, anchored at the sync root.
//!
//! Patterns use gitignore syntax, which is the dialect every contributor
//! already reads in `.gitignore`. The one deliberate difference from both
//! gitignore and rsync is anchoring: see [`anchor`].

use std::collections::HashMap;
use std::path::Path;

use color_eyre::Result;
use color_eyre::eyre::WrapErr as _;
use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// Rewrite a user pattern so it matches only from the sync root.
///
/// gitignore and rsync agree that a pattern with no slash in it floats: `target`
/// matches a `target` entry at every depth. That default is why
/// `--exclude 'result*'` deleted `crates/codec/src/impls/result.rs`. Prefixing a
/// slash pins the pattern to the root, which is what somebody excluding a build
/// artifact by name always meant.
///
/// A pattern that already starts with `/`, or with `**/`, has said which
/// anchoring it wants, so it is passed through untouched. A leading `!`
/// (gitignore's re-include) is preserved and its body anchored.
///
/// ```
/// use tree_sync::filter::anchor;
/// assert_eq!(anchor("result*"), "/result*");
/// assert_eq!(anchor("**/target"), "**/target");
/// assert_eq!(anchor("/build"), "/build");
/// assert_eq!(anchor("!keep.rs"), "!/keep.rs");
/// ```
#[must_use]
pub fn anchor(pattern: &str) -> String {
    let (negation, body) = pattern
        .strip_prefix('!')
        .map_or(("", pattern), |rest| ("!", rest));
    if body.starts_with('/') || body.starts_with("**/") {
        return pattern.to_owned();
    }
    format!("{negation}/{body}")
}

/// One exclude the user asked for, and what it did.
#[derive(Debug, Clone)]
pub struct Rule {
    /// Exactly what the user typed.
    pub given: String,
    /// The pattern actually matched against, after anchoring.
    pub effective: String,
    /// How many paths this rule removed from the file set.
    pub hits: usize,
}

/// A set of excludes that counts what each pattern removed.
#[derive(Debug)]
pub struct Filter {
    matcher: Gitignore,
    rules: Vec<Rule>,
    by_pattern: HashMap<String, usize>,
}

impl Filter {
    /// Build a filter rooted at `root`.
    ///
    /// `anchored` patterns are pinned to the root by [`anchor`]. `any_depth`
    /// patterns are used verbatim, i.e. with rsync's floating behaviour, for the
    /// cases that genuinely want every depth. Later patterns win, so a `!` rule
    /// can re-include something an earlier rule removed.
    ///
    /// # Errors
    /// Returns an error if a pattern is not a valid gitignore glob.
    pub fn new(root: &Path, anchored: &[String], any_depth: &[String]) -> Result<Self> {
        let mut builder = GitignoreBuilder::new(root);
        let mut rules: Vec<Rule> = Vec::new();
        let mut by_pattern = HashMap::new();

        let entries = anchored
            .iter()
            .map(|given| (given, anchor(given)))
            .chain(any_depth.iter().map(|given| (given, given.clone())));

        for (given, effective) in entries {
            builder
                .add_line(None, &effective)
                .wrap_err_with(|| format!("not a usable exclude pattern: {given}"))?;
            by_pattern.insert(effective.clone(), rules.len());
            rules.push(Rule {
                given: given.clone(),
                effective,
                hits: 0,
            });
        }

        let matcher = builder.build().wrap_err("could not build the exclude set")?;
        Ok(Self {
            matcher,
            rules,
            by_pattern,
        })
    }

    /// Whether `relative` is excluded, recording the hit against its rule.
    ///
    /// Ancestors are checked too, so `--exclude docs` removes `docs/api.md`
    /// the way a `.gitignore` entry would, even though the file set is flat.
    pub fn excludes(&mut self, relative: &Path) -> bool {
        let matched = self.matcher.matched_path_or_any_parents(relative, false);
        if !matched.is_ignore() {
            return false;
        }
        if let Some(index) = matched
            .inner()
            .and_then(|glob| self.by_pattern.get(glob.original()))
            .copied()
            && let Some(rule) = self.rules.get_mut(index)
        {
            rule.hits += 1;
        }
        true
    }

    /// The rules with their hit counts, in the order the user gave them.
    #[must_use]
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Whether any exclude was configured at all.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{Filter, anchor};
    use std::path::Path;

    fn filter(patterns: &[&str]) -> Filter {
        let owned: Vec<String> = patterns.iter().map(|p| (*p).to_owned()).collect();
        Filter::new(Path::new("/tree"), &owned, &[]).expect("patterns build")
    }

    /// The bug this tool exists for. `--exclude 'result*'` names the top level
    /// `result` symlink a Nix build leaves behind. Under rsync (and under plain
    /// gitignore) the same pattern floats to every depth and takes
    /// `crates/codec/src/impls/result.rs` with it, which then fails a Rust build
    /// ten minutes later with `file not found for module 'result'`.
    #[test]
    fn result_glob_does_not_reach_into_subdirectories() {
        let mut excludes = filter(&["result*"]);

        assert!(
            excludes.excludes(Path::new("result")),
            "the top level result symlink is what the pattern names"
        );
        assert!(
            excludes.excludes(Path::new("result-doc")),
            "sibling result-* symlinks are named too"
        );
        assert!(
            !excludes.excludes(Path::new("crates/codec/src/impls/result.rs")),
            "a nested source file named result.rs must survive an exclude \
             written for the top level result symlink"
        );
        assert!(
            !excludes.excludes(Path::new("src/result/mod.rs")),
            "a nested directory named result must survive too"
        );
    }

    /// `--exclude target` is the same trap with a different name: it is written
    /// for the Cargo build directory at the root and must not eat a crate's own
    /// `target` module or a vendored `foo/target/` fixture.
    #[test]
    fn target_exclude_stays_at_the_root() {
        let mut excludes = filter(&["target"]);

        assert!(excludes.excludes(Path::new("target/debug/deps/libfoo.rlib")));
        assert!(!excludes.excludes(Path::new("crates/build/src/target.rs")));
        assert!(!excludes.excludes(Path::new("tests/fixtures/target/keep.txt")));
    }

    #[test]
    fn any_depth_is_available_when_asked_for_explicitly() {
        let owned = vec!["**/target".to_owned()];
        let mut excludes = Filter::new(Path::new("/tree"), &owned, &[]).expect("builds");
        assert!(excludes.excludes(Path::new("tests/fixtures/target/keep.txt")));

        let floating = vec!["target".to_owned()];
        let mut excludes = Filter::new(Path::new("/tree"), &[], &floating).expect("builds");
        assert!(excludes.excludes(Path::new("tests/fixtures/target/keep.txt")));
    }

    #[test]
    fn anchor_leaves_explicit_patterns_alone() {
        assert_eq!(anchor("result*"), "/result*");
        assert_eq!(anchor("**/result*"), "**/result*");
        assert_eq!(anchor("/result*"), "/result*");
        assert_eq!(anchor("docs/api"), "/docs/api");
        assert_eq!(anchor("!keep.rs"), "!/keep.rs");
        assert_eq!(anchor("!/keep.rs"), "!/keep.rs");
    }

    #[test]
    fn hits_are_counted_per_rule_so_an_over_broad_exclude_is_visible() {
        let mut excludes = filter(&["docs", "result*"]);

        assert!(excludes.excludes(Path::new("docs/api.md")));
        assert!(excludes.excludes(Path::new("docs/guide/intro.md")));
        assert!(excludes.excludes(Path::new("result")));
        assert!(!excludes.excludes(Path::new("src/main.rs")));

        let rules = excludes.rules();
        assert_eq!(rules[0].given, "docs");
        assert_eq!(rules[0].hits, 2);
        assert_eq!(rules[1].given, "result*");
        assert_eq!(rules[1].hits, 1);
    }

    #[test]
    fn a_pattern_that_matches_nothing_keeps_a_zero_count() {
        let mut excludes = filter(&["typo-here"]);
        assert!(!excludes.excludes(Path::new("src/main.rs")));
        assert_eq!(excludes.rules()[0].hits, 0);
    }
}
