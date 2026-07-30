//! Every lint rule in the format, all of them errors.
//!
//! A rule exists where a memory can be wrong in a way that costs a later reader
//! time: a reference that goes nowhere, evidence that moved, a directory nobody
//! has pruned.

use crate::{
    discover::{Corpus, Scan},
    error::Result,
    model::{BODY_TOKEN_BUDGET, BYTES_PER_TOKEN, Memory, TLDR_MAX_CHARS},
    rank, stale,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::BTreeMap;

/// Memory files in one leaf directory before it is flagged.
///
/// A directory past this is not a memory set anymore, it is an unpruned log, and
/// nobody reads it. Counted per leaf directory rather than per root, so a large
/// corpus splits into grouping subdirectories instead of failing.
///
/// Explicitly **not** evidenced: the only study on this fleet is silent on
/// corpus size, and the external claim behind the number is an unlinked blog
/// citation. Keep it as a forcing function for consolidation; do not defend it
/// as measured.
pub const DIRECTORY_FILE_BUDGET: usize = 150;

/// Days a memory may go unvalidated before it is flagged.
///
/// Half a year: long enough that a stable lesson is not nagged about every
/// sprint, short enough that nothing survives a year unexamined. Placed guess.
pub const UNCHECKED_MAX_DAYS: f64 = 180.0;

/// One lint finding.
///
/// Every rule is an error, so there is no severity field: a warning nobody has
/// to fix is a rule that should not exist yet.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    pub path: String,
    /// 1-based file line where the rule points at a specific line.
    pub line: Option<usize>,
    pub rule: &'static str,
    pub message: String,
}

impl Diagnostic {
    fn new(
        path: &std::path::Path,
        line: Option<usize>,
        rule: &'static str,
        message: String,
    ) -> Self {
        Self {
            path: path.display().to_string(),
            line,
            rule,
            message,
        }
    }

    /// A finding at the line of a frontmatter key.
    ///
    /// This and [`Self::at_file`] replaced the five-line `Diagnostic::new(path,
    /// key_line(memory, KEY), RULE, format!(..))` block that every single-value
    /// rule wrote out. One call site each means the `path` and line lookup are
    /// stated once, so a change to either is one edit rather than eight.
    fn at_key(memory: &Memory, key: &str, rule: &'static str, message: String) -> Self {
        Self::new(memory.path.as_path(), key_line(memory, key), rule, message)
    }

    /// A finding about the file as a whole, with no line to point at.
    fn at_file(memory: &Memory, rule: &'static str, message: String) -> Self {
        Self::new(memory.path.as_path(), None, rule, message)
    }

    /// Human-readable single line: `path:line rule: message`.
    #[must_use]
    pub fn render(&self) -> String {
        let location = self
            .line
            .map_or_else(|| self.path.clone(), |line| format!("{}:{line}", self.path));
        format!("{location} {}: {}", self.rule, self.message)
    }
}

/// Lint every memory in the corpus.
///
/// # Errors
///
/// Returns an error when a `based_on` path cannot be checked: a malformed glob,
/// or an IO failure that is not "not found".
pub fn lint(corpus: &Corpus, now: DateTime<Utc>) -> Result<Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();

    // A file that did not parse is reported with the rule its parse failure
    // belongs to. Never dropped: an unparsed memory that says nothing is the
    // failure mode this format exists to avoid.
    for failure in &corpus.failures {
        diagnostics.push(Diagnostic::new(
            &failure.path,
            failure.line,
            failure.rule,
            failure.message.clone(),
        ));
    }

    // Credentials are found while reading, so this covers files that did not
    // parse as well as the ones that did.
    for found in &corpus.secrets {
        diagnostics.push(Diagnostic::new(
            &found.path,
            Some(found.finding.line),
            "memory-secret",
            format!(
                "line holds what the fleet's redaction table reads as a credential ({kinds}); \
                 a memory is committed on purpose, so remove it and rotate the secret",
                kinds = found.finding.kinds.join(", ")
            ),
        ));
    }

    for scan in &corpus.scans {
        for &index in &scan.memories {
            let memory = &corpus.memories[index];
            check_memory(corpus, scan, memory, now, &mut diagnostics)?;
        }
        check_directory(corpus, scan, &mut diagnostics);
    }

    check_duplicate_tldrs(corpus, &mut diagnostics);

    // Grouped by file, then by line. A diagnostic about the file as a whole
    // sorts after the ones pointing at a line, so a reader works down the file
    // and then reads the whole-file findings.
    diagnostics.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| {
                left.line
                    .unwrap_or(usize::MAX)
                    .cmp(&right.line.unwrap_or(usize::MAX))
            })
            .then_with(|| left.rule.cmp(right.rule))
            .then_with(|| left.message.cmp(&right.message))
    });
    Ok(diagnostics)
}

/// One list-shaped rule: the frontmatter key whose line a finding points at, and
/// the rule it is reported under.
struct ListRule {
    key: &'static str,
    rule: &'static str,
}

const TOPIC_UNKNOWN: ListRule = ListRule {
    key: "topic",
    rule: "memory-topic-unknown",
};
const RELATED_UNRESOLVED: ListRule = ListRule {
    key: "related",
    rule: "memory-related-unresolved",
};
const SUPERSEDES_UNRESOLVED: ListRule = ListRule {
    key: "supersedes",
    rule: "memory-supersedes-unresolved",
};
const BASED_ON_MISSING: ListRule = ListRule {
    key: "based_on",
    rule: "memory-based-on-missing",
};

/// Report every entry of one frontmatter list that fails `holds`.
///
/// This replaced four near-identical loops (`topic`, `related`, `supersedes`,
/// `based_on`) that differed only in the key they read, the rule they named and
/// the sentence they wrote. One function beats four copies for the ordinary
/// reason: the next list-shaped rule is a call rather than a paste, and a wrong
/// line number or a changed `Diagnostic` shape is one edit instead of four. The
/// predicate is fallible because `based_on` has to touch the filesystem, and
/// splitting on that would have kept two of the four.
fn check_list<T>(
    memory: &Memory,
    rule: &ListRule,
    values: &[T],
    holds: impl Fn(&T) -> Result<bool>,
    message: impl Fn(&T) -> String,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    for value in values {
        if !holds(value)? {
            diagnostics.push(Diagnostic::new(
                memory.path.as_path(),
                key_line(memory, rule.key),
                rule.rule,
                message(value),
            ));
        }
    }
    Ok(())
}

fn check_memory(
    corpus: &Corpus,
    scan: &Scan,
    memory: &Memory,
    now: DateTime<Utc>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    check_values(memory, now, diagnostics);

    // Absent `topics.txt` means any topic is allowed, so there is nothing to
    // check rather than nothing that passes.
    if let Some(closed_set) = &scan.topics {
        check_list(
            memory,
            &TOPIC_UNKNOWN,
            &memory.topic,
            |topic| Ok(closed_set.contains(topic)),
            |topic| format!("topic {topic:?} is not in the closed set"),
            diagnostics,
        )?;
    }

    // `related` and `supersedes` are the same rule over two lists: a slug that
    // names nothing. Driven from a pair rather than written twice, so the sentence
    // exists once and the rule's own key names the list it read.
    for (rule, slugs) in [
        (&RELATED_UNRESOLVED, &memory.related),
        (&SUPERSEDES_UNRESOLVED, &memory.supersedes),
    ] {
        check_list(
            memory,
            rule,
            slugs,
            |slug| Ok(corpus.resolves(slug)),
            |slug| {
                format!(
                    "`{key}` names {slug:?}, which is not a memory in any root",
                    key = rule.key
                )
            },
            diagnostics,
        )?;
    }

    check_list(
        memory,
        &BASED_ON_MISSING,
        &memory.based_on,
        |entry| stale::resolves(&memory.root, entry),
        |entry| {
            format!(
                "`based_on` path {target:?} does not exist under {root}",
                target = entry.path,
                root = memory.root.display(),
            )
        },
        diagnostics,
    )?;

    Ok(())
}

/// Every single-value rule as a pure "what is wrong, if anything".
///
/// Splitting the fault from the reporting is the same move [`check_list`] makes
/// for the list rules, for the same reason: each rule became a five-line
/// push-a-`Diagnostic` block that differed only in the sentence, so the reporting
/// is written once per rule as a single line and the judgement is a function that
/// can be read, and tested, on its own.
fn tldr_fault(memory: &Memory) -> Option<String> {
    if memory.tldr.trim().is_empty() {
        return Some("`tldr` is empty; it is the line a reader decides on".to_owned());
    }
    let chars = memory.tldr.chars().count();
    (chars > TLDR_MAX_CHARS)
        .then(|| format!("`tldr` is {chars} chars, over the {TLDR_MAX_CHARS}-char ceiling"))
}

fn body_budget_fault(memory: &Memory) -> Option<String> {
    let estimated_tokens = memory.body.len() / BYTES_PER_TOKEN;
    (estimated_tokens > BODY_TOKEN_BUDGET).then(|| {
        format!(
            "body is ~{estimated_tokens} estimated tokens, over the \
             {BODY_TOKEN_BUDGET}-token budget for `genre: memory`"
        )
    })
}

fn stem_fault(memory: &Memory) -> Option<String> {
    (!crate::model::is_kebab_case(&memory.slug))
        .then(|| format!("file stem {slug:?} is not kebab-case", slug = memory.slug))
}

fn written_slug_fault(memory: &Memory) -> Option<String> {
    memory.frontmatter_slug.as_ref().map(|written| {
        format!(
            "frontmatter writes `slug: {written}`; the slug is the file stem and is never \
             written in the frontmatter"
        )
    })
}

/// Why nobody can vouch for this memory, if nobody can.
fn unchecked_fault(memory: &Memory, now: DateTime<Utc>) -> Option<String> {
    let reason = memory.newest_validation().map_or_else(
        || Some("has no `validated` entry".to_owned()),
        |newest| {
            let age_days = rank::days_between(newest.at_time, now);
            (age_days > UNCHECKED_MAX_DAYS).then(|| {
                format!("was last validated {age_days:.0} days ago, over {UNCHECKED_MAX_DAYS:.0}")
            })
        },
    )?;
    Some(format!(
        "{reason}; re-run its proof and record it with `memories validate`, or let it be a \
         `living` or `historical` page"
    ))
}

/// Report every single-value rule: the ones that hold for any memory, then the
/// two that hold for `genre: memory` only.
///
/// The genre guard is written once rather than at the top of each rule, which is
/// what removes the need for an `evergreen` escape hatch: a reference page is
/// supposed to be long and nobody re-proves it, so both the body budget and the
/// validation clock are about memories specifically, and an exception you have to
/// remember to set is a field that gets forgotten.
fn check_values(memory: &Memory, now: DateTime<Utc>, diagnostics: &mut Vec<Diagnostic>) {
    push_at_key(
        memory,
        "tldr",
        "memory-tldr",
        tldr_fault(memory),
        diagnostics,
    );
    push_at_file(memory, "memory-slug", stem_fault(memory), diagnostics);
    push_at_key(
        memory,
        "slug",
        "memory-slug",
        written_slug_fault(memory),
        diagnostics,
    );

    if memory.genre != crate::model::Genre::Memory {
        return;
    }
    push_at_file(
        memory,
        "memory-body-budget",
        body_budget_fault(memory),
        diagnostics,
    );
    push_at_file(
        memory,
        "memory-unchecked",
        unchecked_fault(memory, now),
        diagnostics,
    );
}

/// Record a fault, if there is one, at the line of a frontmatter key.
fn push_at_key(
    memory: &Memory,
    key: &str,
    rule: &'static str,
    fault: Option<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(message) = fault {
        diagnostics.push(Diagnostic::at_key(memory, key, rule, message));
    }
}

/// Record a fault, if there is one, against the file as a whole.
fn push_at_file(
    memory: &Memory,
    rule: &'static str,
    fault: Option<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(message) = fault {
        diagnostics.push(Diagnostic::at_file(memory, rule, message));
    }
}

fn check_directory(corpus: &Corpus, scan: &Scan, diagnostics: &mut Vec<Diagnostic>) {
    for leaf in &scan.leaves {
        if leaf.files > DIRECTORY_FILE_BUDGET {
            diagnostics.push(Diagnostic::new(
                &leaf.dir,
                None,
                "memory-directory-budget",
                format!(
                    "{files} memory files, over the {DIRECTORY_FILE_BUDGET}-file budget",
                    files = leaf.files
                ),
            ));
        }
    }

    // The slug is the file stem, never the path, so two files with the same stem
    // in one root are one slug with two meanings and `show` can only ever return
    // one of them.
    let mut seen: BTreeMap<&str, &Memory> = BTreeMap::new();
    for &index in &scan.memories {
        let memory = &corpus.memories[index];
        match seen.get(memory.slug.as_str()) {
            None => {
                seen.insert(memory.slug.as_str(), memory);
            }
            Some(first) => diagnostics.push(Diagnostic::at_file(
                memory,
                "memory-stem-collision",
                format!(
                    "same stem as {other}, and the slug is the stem rather than the path",
                    other = first.path.display()
                ),
            )),
        }
    }
}

/// Report a `tldr` that appears more than once in the corpus.
///
/// Two memories with the same `tldr` are the same memory written twice, and a
/// search that returns both wastes the reader's attention. Reported on every
/// copy after the first, naming the one it duplicates.
fn check_duplicate_tldrs(corpus: &Corpus, diagnostics: &mut Vec<Diagnostic>) {
    let mut seen: BTreeMap<&str, &Memory> = BTreeMap::new();
    for memory in &corpus.memories {
        let key = memory.tldr.trim();
        if key.is_empty() {
            continue;
        }
        match seen.get(key) {
            None => {
                seen.insert(key, memory);
            }
            Some(first) => diagnostics.push(Diagnostic::at_key(
                memory,
                "tldr",
                "memory-duplicate-tldr",
                format!("same `tldr` as {other}", other = first.path.display()),
            )),
        }
    }
}

/// File line of a top-level frontmatter key, so a diagnostic points at the
/// offending line rather than at the file. The calculation itself lives in
/// [`crate::model::key_line`], which the parser needs too.
fn key_line(memory: &Memory, key: &str) -> Option<usize> {
    crate::model::key_line(&memory.yaml, memory.yaml_start_line, key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{Repo, fixed_now};

    fn rules(diagnostics: &[Diagnostic]) -> Vec<&'static str> {
        diagnostics.iter().map(|d| d.rule).collect()
    }

    fn lint_repo(repo: &Repo) -> Vec<Diagnostic> {
        lint(&repo.load(), fixed_now()).expect("linting a fixture corpus")
    }

    /// Lint the fixture and assert exactly these rules fired, in order.
    ///
    /// One helper rather than the same three-line lint-then-assert-then-format
    /// block in every test: the assertion shape is stated once, so a change to
    /// what a failure prints is one edit. Returns the diagnostics so a test can
    /// go on to check a message.
    fn expect_rules(repo: &Repo, expected: &[&str]) -> Vec<Diagnostic> {
        let diagnostics = lint_repo(repo);
        assert_eq!(rules(&diagnostics), expected, "{diagnostics:?}");
        diagnostics
    }

    /// Assert the first diagnostic's message names `needle`, which is what makes
    /// a rule actionable rather than merely correct.
    fn expect_message(diagnostics: &[Diagnostic], needle: &str) {
        assert!(
            diagnostics
                .first()
                .is_some_and(|diagnostic| diagnostic.message.contains(needle)),
            "expected a message naming {needle:?}: {diagnostics:?}"
        );
    }

    #[test]
    fn a_clean_memory_produces_no_diagnostics() {
        let repo = Repo::new();
        repo.memory("nix-rebuild-cascade", "validated_today", "Body.\n");
        let diagnostics = lint_repo(&repo);
        assert!(
            diagnostics.is_empty(),
            "expected clean, got {diagnostics:?}"
        );
    }

    #[test]
    fn unparsed_files_are_reported_rather_than_skipped() {
        let repo = Repo::new();
        repo.raw("no-fence.md", "Just a body.\n");
        repo.raw("unterminated.md", "---\ntldr: A line\n");
        repo.raw("empty-frontmatter.md", "---\n---\nBody.\n");
        expect_rules(&repo, &["memory-frontmatter"; 3]);
    }

    #[test]
    fn empty_and_overlong_tldrs_are_flagged() {
        let repo = Repo::new();
        repo.raw(
            "empty-tldr.md",
            "---\ntldr: \"\"\ngenre: living\n---\nBody.\n",
        );
        let long = "x".repeat(TLDR_MAX_CHARS + 1);
        repo.raw(
            "long-tldr.md",
            &format!("---\ntldr: {long}\ngenre: living\n---\nBody.\n"),
        );
        expect_rules(&repo, &["memory-tldr", "memory-tldr"]);
    }

    #[test]
    fn body_budget_applies_to_genre_memory_only() {
        let repo = Repo::new();
        let long_body = "x".repeat((BODY_TOKEN_BUDGET + 1) * BYTES_PER_TOKEN);
        repo.memory("long-memory", "validated_today", &long_body);
        repo.memory("long-living", "genre: living\nvalidated_today", &long_body);
        let diagnostics = expect_rules(&repo, &["memory-body-budget"]);
        assert!(
            diagnostics[0].path.ends_with("long-memory.md"),
            "only `genre: memory` carries the budget: {diagnostics:?}"
        );
    }

    #[test]
    fn a_written_slug_and_a_non_kebab_stem_are_both_slug_errors() {
        let repo = Repo::new();
        repo.memory("Not_Kebab", "validated_today", "Body.\n");
        repo.memory("has-slug", "slug: elsewhere\nvalidated_today", "Body.\n");
        expect_rules(&repo, &["memory-slug", "memory-slug"]);
    }

    #[test]
    fn topics_are_checked_only_when_a_closed_set_exists() {
        let repo = Repo::new();
        repo.memory("with-topic", "topic: [nixos]\nvalidated_today", "Body.\n");
        assert!(
            lint_repo(&repo).is_empty(),
            "no topics.txt means any topic is allowed"
        );

        repo.topics(&["nix", "builds"]);
        let diagnostics = expect_rules(&repo, &["memory-topic-unknown"]);
        expect_message(&diagnostics, "nixos");
    }

    #[test]
    fn unresolved_related_and_supersedes_are_separate_rules() {
        let repo = Repo::new();
        repo.memory(
            "a-memory",
            "related: [gone]\nsupersedes: [also-gone]\nvalidated_today",
            "Body.\n",
        );
        expect_rules(
            &repo,
            &["memory-related-unresolved", "memory-supersedes-unresolved"],
        );
    }

    #[test]
    fn a_based_on_path_that_no_longer_exists_is_an_error() {
        let repo = Repo::new();
        repo.memory(
            "based-on-gone",
            "based_on:\n  - path: src/gone.rs\nvalidated_today",
            "Body.\n",
        );
        let diagnostics = expect_rules(&repo, &["memory-based-on-missing"]);
        expect_message(&diagnostics, "src/gone.rs");
    }

    #[test]
    fn duplicate_tldrs_are_reported_on_the_later_copy() {
        let repo = Repo::new();
        repo.raw(
            "first.md",
            "---\ntldr: The very same line\ngenre: living\n---\nBody.\n",
        );
        repo.raw(
            "second.md",
            "---\ntldr: The very same line\ngenre: living\n---\nOther body.\n",
        );
        let diagnostics = expect_rules(&repo, &["memory-duplicate-tldr"]);
        assert!(
            diagnostics[0].path.ends_with("second.md"),
            "{diagnostics:?}"
        );
        assert!(
            diagnostics[0].message.contains("first.md"),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn unchecked_covers_both_never_validated_and_long_ago() {
        let repo = Repo::new();
        repo.memory("never-validated", "", "Body.\n");
        repo.memory(
            "long-ago",
            "validated:\n  - at: 2020-01-01T00:00:00Z\n    by: t\n    how: c\n    ok: true\n",
            "Body.\n",
        );
        repo.memory("a-living-page", "genre: living\n", "Body.\n");
        expect_rules(&repo, &["memory-unchecked", "memory-unchecked"]);
    }

    /// `always:` is not a field of this format, and the rule that says so is the
    /// only thing stopping it from coming back by habit.
    #[test]
    fn a_retired_key_is_an_unknown_key_rather_than_an_ignored_line() {
        let repo = Repo::new();
        repo.memory("was-always", "always: true\ngenre: living\n", "Body.\n");
        let diagnostics = expect_rules(&repo, &["memory-unknown-key"]);
        expect_message(&diagnostics, "always");
    }

    #[test]
    fn two_files_with_one_stem_in_a_root_collide() {
        let repo = Repo::new();
        repo.memory("shared-stem", "genre: living\n", "One.\n");
        repo.group_memory("cas", "shared-stem", "genre: living\n", "Two.\n");
        let diagnostics = lint_repo(&repo);
        assert!(
            rules(&diagnostics).contains(&"memory-stem-collision"),
            "{diagnostics:?}"
        );
    }

    /// The rule with an incident behind it: a live Linear key that reached 200
    /// indexed chunks. `validated.how` holds a command line, which is the shape
    /// that leaked.
    /// `validated.how` holds a command line, which is exactly the shape that
    /// leaked into 200 indexed chunks on this fleet, so the realistic `curl -H
    /// "Authorization: lin_api_..."` value is the case that matters most.
    #[test]
    fn a_credential_in_a_validated_how_is_reported_with_its_line() {
        let repo = Repo::new();
        repo.memory(
            "leaky-how",
            "genre: living\nvalidated:\n  - at: 2026-07-29T00:00:00Z\n    by: t\n    \
             how: 'curl -H \"Authorization: lin_api_abc123\" https://api.linear.app/graphql'\n    \
             ok: true\n",
            "Body.\n",
        );
        let diagnostics = lint_repo(&repo);
        assert_eq!(rules(&diagnostics), ["memory-secret"], "{diagnostics:?}");
        // Fence, tldr, genre, `validated:`, `at:`, `by:`, then `how:`.
        assert_eq!(
            diagnostics[0].line,
            Some(7),
            "the `how:` line itself: {diagnostics:?}"
        );
        assert!(
            diagnostics[0].message.contains("linear_api_key"),
            "the message names the credential kind: {diagnostics:?}"
        );
    }

    /// The body half of the same rule, asserted separately so neither case can
    /// pass by standing in for the other.
    #[test]
    fn a_credential_in_the_body_is_reported_with_its_line() {
        let repo = Repo::new();
        repo.memory(
            "leaky-body",
            "genre: living\n",
            "The key was AKIA0123456789ABCDEF and it is now rotated.\n",
        );
        let diagnostics = lint_repo(&repo);
        assert_eq!(rules(&diagnostics), ["memory-secret"], "{diagnostics:?}");
        // Fence, tldr, genre, closing fence, then the first body line.
        assert_eq!(
            diagnostics[0].line,
            Some(5),
            "the first body line: {diagnostics:?}"
        );
        assert!(
            diagnostics[0].message.contains("aws_access_key_id"),
            "the message names the credential kind: {diagnostics:?}"
        );
    }

    #[test]
    fn the_file_budget_counts_each_leaf_directory_rather_than_the_root() {
        let repo = Repo::new();
        for index in 0..=DIRECTORY_FILE_BUDGET {
            repo.group_memory(
                "docs",
                &format!("page-{index}"),
                "genre: living\n",
                "Body.\n",
            );
        }
        let diagnostics = expect_rules(&repo, &["memory-directory-budget"]);
        assert!(
            diagnostics[0].path.ends_with("docs"),
            "the budget belongs to the leaf directory: {diagnostics:?}"
        );
    }

    /// 300 memories across twenty leaves of fifteen is not an unpruned log, and
    /// the budget exists to force that split rather than to cap a root.
    #[test]
    fn a_root_far_over_the_budget_across_small_leaves_passes() {
        let repo = Repo::new();
        for group in 0..20 {
            for index in 0..15 {
                repo.group_memory(
                    &format!("group-{group}"),
                    &format!("page-{group}-{index}"),
                    "genre: living\n",
                    "Body.\n",
                );
            }
        }
        let diagnostics = lint_repo(&repo);
        assert!(
            diagnostics.is_empty(),
            "300 files across leaves of 15 is clean: {diagnostics:?}"
        );
    }

    /// A file two levels deep cannot be found by discovery, and a corpus that
    /// drops it silently is the failure the format is built to avoid.
    #[test]
    fn a_file_buried_below_one_level_is_reported_rather_than_dropped() {
        let repo = Repo::new();
        repo.buried_memory("docs/deeper", "too-deep", "genre: living\n", "Body.\n");
        let diagnostics = expect_rules(&repo, &["memory-slug"]);
        expect_message(&diagnostics, "one level");
    }
}
