//! The on-disk memory format: split, parse, and render frontmatter.
//!
//! Every failure is a typed [`ParseError`] carrying the lint rule it belongs
//! to, so nothing is ever skipped silently.
//!
//! Cursor's `.mdc` rules drop a file whose frontmatter is malformed and say
//! nothing, which is a documented way to lose a rule you believe is loaded.
//! Here a file that does not parse is a diagnostic with a line number.

use crate::error::{self, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use snafu::ResultExt;
use std::path::{Path, PathBuf};

/// Extension every memory file carries.
///
/// Anything else in a `.memories` directory (a `topics.txt`, a README) is not a
/// memory and is not scanned.
pub const MEMORY_EXTENSION: &str = "md";

/// `tldr` character ceiling.
///
/// A `tldr` is what gets injected into a model context to decide whether to
/// read the body, so an overlong one is wasted budget. Same value as
/// `skill-lint`'s `DESCRIPTION_MAX_CHARS`
/// (`packages/skill-lint/src/lint.rs:14`), which defends the same thing;
/// reused rather than picked again.
pub const TLDR_MAX_CHARS: usize = 1024;

/// Estimated-token ceiling on a `genre: memory` body.
///
/// Counted at [`BYTES_PER_TOKEN`] bytes per token. A memory is meant to be read
/// whole by an agent that found it, so a body past this is a document and
/// belongs in `doc/`. Same value as `skill-lint`'s `FILE_TOKEN_BUDGET`
/// (`packages/skill-lint/src/lint.rs:19`).
pub const BODY_TOKEN_BUDGET: usize = 3000;

/// Bytes-per-token divisor for the estimate above: the rough English/Markdown
/// ratio, used to avoid a tokenizer dependency for a budget check.
pub const BYTES_PER_TOKEN: usize = 4;

/// `prior` when the frontmatter omits it: an even bet. Written once at birth,
/// never edited, so this default only applies to a memory whose author had no
/// opinion.
pub const DEFAULT_PRIOR: f64 = 0.5;

/// Hex characters of the `blake3` digest recorded for a `based_on` entry.
///
/// A full 64-character digest is unreadable in a diff, and this is edit
/// detection rather than an adversarial check, so 64 bits is far more than the
/// job needs. A placed guess, not a measurement. Comparison is by common
/// prefix, so a hand-shortened value (the contract's example is 10 characters)
/// still validates.
pub const BASED_ON_HASH_HEX_CHARS: usize = 16;

/// What a memory is for. Drives the ranking penalty in
/// [`crate::rank`] and which lint rules apply.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum Genre {
    /// A durable lesson: the default, and the only genre under the body budget.
    #[default]
    Memory,
    /// A page kept current by hand; expected to change.
    Living,
    /// A procedure to follow rather than a fact to know.
    Recipe,
    /// True once, kept for the record. Ranked down.
    Historical,
    /// Deliberately frozen, not to be updated. Ranked down.
    Frozen,
}

/// Who a memory is for.
///
/// `shared` is the default and the normal case. `user:<name>` marks a memory
/// that is one person's, kept in a shared directory. There is deliberately no
/// `always:` and no session-start injection: a memory reaches a model because
/// something searched for it (`CONTRACT.md`, "Nothing is injected unasked").
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Scope {
    #[default]
    Shared,
    User(String),
}

impl Scope {
    /// Parse the `scope:` value. `shared`, or `user:<name>` with a non-empty
    /// name.
    ///
    /// # Errors
    ///
    /// Returns the offending value when it is neither.
    pub fn parse(value: &str) -> std::result::Result<Self, &str> {
        if value == "shared" {
            return Ok(Self::Shared);
        }
        match value.strip_prefix("user:") {
            Some(name) if !name.is_empty() => Ok(Self::User(name.to_owned())),
            _ => Err(value),
        }
    }

    /// The `scope:` value as written on disk and in JSON.
    #[must_use]
    pub fn rendered(&self) -> String {
        match self {
            Self::Shared => "shared".to_owned(),
            Self::User(name) => format!("user:{name}"),
        }
    }
}

impl Serialize for Scope {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.rendered())
    }
}

/// Every frontmatter key the format defines.
///
/// A key outside this set is `memory-unknown-key`, which is what catches
/// `always:` and `owns:` coming back by habit. `slug` is in the set only so the
/// linter can report it by its own rule: the slug is the file stem and the
/// format never writes it.
pub const KNOWN_KEYS: [&str; 11] = [
    "tldr",
    "genre",
    "topic",
    "handle",
    "prior",
    "related",
    "based_on",
    "validated",
    "supersedes",
    "scope",
    "slug",
];

/// One `validated:` entry. `at` is kept as written so JSON output and rewrites
/// reproduce the file byte for byte; `at_time` is the same instant parsed.
#[derive(Clone, Debug, Serialize)]
pub struct Validated {
    pub at: String,
    pub by: String,
    pub how: String,
    pub ok: bool,
    #[serde(skip)]
    pub at_time: DateTime<Utc>,
}

/// One `based_on:` entry: a repo-relative path (or glob) and the content hash
/// it had when the memory was last validated.
#[derive(Clone, Debug)]
pub struct BasedOn {
    pub path: String,
    pub blake3: Option<String>,
}

impl BasedOn {
    /// Whether the path is a glob pattern rather than one file. A glob carries
    /// no hash: there is no single content to hash.
    #[must_use]
    pub fn is_glob(&self) -> bool {
        self.path.contains(['*', '?', '['])
    }
}

/// A parsed memory file. Field-for-field the frontmatter plus the body, with
/// the identity (`slug`, `path`, `root`) the loader resolved.
#[derive(Clone, Debug)]
pub struct Memory {
    /// File stem. Never written in the frontmatter.
    pub slug: String,
    /// Absolute path of the file.
    pub path: PathBuf,
    /// Absolute path of the directory holding the `.memories` directory.
    pub root: PathBuf,
    pub tldr: String,
    pub genre: Genre,
    pub topic: Vec<String>,
    pub handle: Vec<String>,
    pub prior: f64,
    pub related: Vec<String>,
    pub based_on: Vec<BasedOn>,
    pub validated: Vec<Validated>,
    pub supersedes: Vec<String>,
    pub scope: Scope,
    pub body: String,
    /// A `slug:` key found in the frontmatter, which the format forbids. Kept
    /// rather than rejected at parse time so the linter can name it precisely.
    pub frontmatter_slug: Option<String>,
    /// The exact YAML block, for line-accurate diagnostics.
    pub yaml: String,
    /// 1-based file line the YAML block starts on.
    pub yaml_start_line: usize,
}

impl Memory {
    /// Newest `validated` entry by timestamp. Ties go to the one written last,
    /// so appending is always what wins.
    #[must_use]
    pub fn newest_validation(&self) -> Option<&Validated> {
        self.validated.iter().reduce(|newest, entry| {
            if entry.at_time >= newest.at_time {
                entry
            } else {
                newest
            }
        })
    }

    /// Refuted: the newest validation says the memory did not hold.
    #[must_use]
    pub fn is_refuted(&self) -> bool {
        self.newest_validation().is_some_and(|entry| !entry.ok)
    }

    /// Number of confirmations. Feeds the logarithmic reinforcement term in
    /// [`crate::rank::score`].
    #[must_use]
    pub fn ok_count(&self) -> usize {
        self.validated.iter().filter(|entry| entry.ok).count()
    }
}

/// A file that is not a memory: which lint rule it broke, where, and why.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseError {
    pub rule: &'static str,
    pub line: Option<usize>,
    pub message: String,
}

impl ParseError {
    fn new(rule: &'static str, line: Option<usize>, message: impl Into<String>) -> Self {
        Self {
            rule,
            line,
            message: message.into(),
        }
    }
}

/// Frontmatter split from a memory file.
#[derive(Debug, Eq, PartialEq)]
pub struct Sections<'a> {
    /// YAML text between the fences, exactly as written.
    pub yaml: &'a str,
    /// Everything after the closing fence, exactly as written.
    pub body: &'a str,
    /// 1-based file line the YAML block starts on.
    pub yaml_start_line: usize,
}

/// Why a file has no usable frontmatter. The two cases have different fixes
/// (add a fence vs close the one you opened), so they are different
/// diagnostics rather than one "malformed" catch-all.
#[derive(Debug, Eq, PartialEq)]
pub enum SplitFailure {
    /// The file does not open with a bare `---` line.
    NoFence,
    /// It opens with `---` but no later line closes the block.
    Unterminated,
}

/// Split a leading `---` … `---` block, returning the YAML and the body.
///
/// Byte spans are tracked with `split_inclusive`, so both slices are exact and
/// a `\r\n` file is not silently rewritten by a reconstruction that assumes
/// `\n`.
///
/// # Errors
///
/// Returns [`SplitFailure::NoFence`] when the file does not open with a bare
/// `---` line and [`SplitFailure::Unterminated`] when nothing closes the block.
pub fn split_sections(contents: &str) -> std::result::Result<Sections<'_>, SplitFailure> {
    let Some(first_line) = contents.lines().next() else {
        return Err(SplitFailure::NoFence);
    };
    if first_line.trim_end() != "---" {
        return Err(SplitFailure::NoFence);
    }
    // A file that is only `---` with no newline opened a block it never closed.
    let Some(newline_index) = contents.find('\n') else {
        return Err(SplitFailure::Unterminated);
    };

    let rest = &contents[newline_index + 1..];
    let mut offset = 0usize;
    for chunk in rest.split_inclusive('\n') {
        if chunk.trim_end() == "---" {
            return Ok(Sections {
                yaml: &rest[..offset],
                body: &rest[offset + chunk.len()..],
                // File line 1 is the opening fence, so the YAML starts on 2.
                yaml_start_line: 2,
            });
        }
        offset += chunk.len();
    }
    Err(SplitFailure::Unterminated)
}

/// The frontmatter as serde sees it.
///
/// `deny_unknown_fields` on purpose: a typo (`topics:` for `topic:`) that parses
/// as an unknown key is exactly the silent loss this format exists to avoid, so
/// an unrecognized key is an error rather than an ignored line. `slug` is
/// accepted here only so the linter can report it by name instead of it
/// surfacing as an opaque unknown key.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFrontmatter {
    tldr: Option<String>,
    genre: Option<Genre>,
    #[serde(default)]
    topic: Vec<String>,
    #[serde(default)]
    handle: Vec<String>,
    prior: Option<f64>,
    #[serde(default)]
    related: Vec<String>,
    #[serde(default)]
    based_on: Vec<RawBasedOn>,
    #[serde(default)]
    validated: Vec<RawValidated>,
    #[serde(default)]
    supersedes: Vec<String>,
    scope: Option<String>,
    slug: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBasedOn {
    path: String,
    blake3: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawValidated {
    at: String,
    by: String,
    how: String,
    ok: bool,
}

/// Parse one memory file. `path` is absolute and `root` is the directory
/// holding its `.memories`.
///
/// # Errors
///
/// Returns a [`ParseError`] naming the lint rule and, where the YAML parser
/// gives one, the file line: a missing or unterminated fence, empty or
/// non-mapping frontmatter, an unknown key, a value of the wrong type, an
/// out-of-range `prior`, an unparseable `validated.at`, a non-hex `blake3`,
/// or a missing `tldr`.
pub fn parse_memory(
    path: &Path,
    root: &Path,
    contents: &str,
) -> std::result::Result<Memory, ParseError> {
    let Some(slug) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return Err(ParseError::new(
            "memory-slug",
            None,
            "file name is not valid UTF-8, so it cannot be a slug",
        ));
    };

    let sections = split_sections(contents).map_err(|failure| match failure {
        SplitFailure::NoFence => ParseError::new(
            "memory-frontmatter",
            Some(1),
            "no frontmatter: the file must open with a bare `---` line",
        ),
        SplitFailure::Unterminated => ParseError::new(
            "memory-frontmatter",
            Some(1),
            "unterminated frontmatter: the opening `---` has no closing `---` line",
        ),
    })?;

    let mapping = check_shape(&sections)?;
    // Before serde looks at the fields, so an unrecognized key gets its own rule
    // rather than surfacing as a generic frontmatter parse error.
    check_known_keys(&mapping, &sections)?;

    let raw: RawFrontmatter = serde_norway::from_str(sections.yaml).map_err(|yaml_error| {
        ParseError::new(
            "memory-frontmatter",
            yaml_line(&yaml_error, sections.yaml_start_line),
            format!("invalid frontmatter: {yaml_error}"),
        )
    })?;

    let Some(tldr) = raw.tldr else {
        return Err(ParseError::new(
            "memory-tldr",
            Some(sections.yaml_start_line),
            "frontmatter has no `tldr`, the one field every memory must carry",
        ));
    };

    let prior = raw.prior.unwrap_or(DEFAULT_PRIOR);
    if !(0.0..=1.0).contains(&prior) {
        return Err(ParseError::new(
            "memory-frontmatter",
            key_line(&sections, "prior"),
            format!("`prior` is {prior}, outside the 0..1 range"),
        ));
    }

    let validated = parse_validations(raw.validated, &sections)?;
    let based_on = parse_based_on(raw.based_on, &sections)?;

    let scope = match &raw.scope {
        None => Scope::Shared,
        Some(written) => Scope::parse(written).map_err(|bad| {
            ParseError::new(
                "memory-frontmatter",
                key_line(&sections, "scope"),
                format!("`scope` is {bad:?}, not `shared` or `user:<name>`"),
            )
        })?,
    };

    Ok(Memory {
        slug: slug.to_owned(),
        path: path.to_path_buf(),
        root: root.to_path_buf(),
        tldr,
        genre: raw.genre.unwrap_or_default(),
        topic: raw.topic,
        handle: raw.handle,
        prior,
        related: raw.related,
        based_on,
        validated,
        supersedes: raw.supersedes,
        scope,
        body: sections.body.to_owned(),
        frontmatter_slug: raw.slug,
        yaml: sections.yaml.to_owned(),
        yaml_start_line: sections.yaml_start_line,
    })
}

/// Reject frontmatter that is not a non-empty YAML mapping, before serde looks
/// at the fields. "Empty" and "not a mapping" then get their own messages
/// instead of serde's field-level complaint about the whole document.
fn check_shape(sections: &Sections<'_>) -> std::result::Result<serde_norway::Mapping, ParseError> {
    let value: serde_norway::Value =
        serde_norway::from_str(sections.yaml).map_err(|yaml_error| {
            ParseError::new(
                "memory-frontmatter",
                yaml_line(&yaml_error, sections.yaml_start_line),
                format!("invalid YAML frontmatter: {yaml_error}"),
            )
        })?;

    let complaint = match value {
        serde_norway::Value::Mapping(mapping) if !mapping.is_empty() => return Ok(mapping),
        serde_norway::Value::Null => {
            "frontmatter is empty: it declares no keys, and `tldr` is required"
        }
        serde_norway::Value::Mapping(_) => {
            "frontmatter is an empty mapping: it declares no keys, and `tldr` is required"
        }
        _ => "frontmatter must be a YAML mapping of `key: value` pairs",
    };
    Err(ParseError::new(
        "memory-frontmatter",
        Some(sections.yaml_start_line),
        complaint,
    ))
}

/// Reject a frontmatter key the format does not define.
///
/// This is the rule that keeps a mistyped or retired key from being silently
/// ignored: `topics:` for `topic:` is a memory that never matches the topic
/// filter its author set, and `always:` is a field this format deliberately does
/// not have.
fn check_known_keys(
    mapping: &serde_norway::Mapping,
    sections: &Sections<'_>,
) -> std::result::Result<(), ParseError> {
    for key in mapping.keys() {
        let Some(name) = key.as_str() else {
            return Err(ParseError::new(
                "memory-unknown-key",
                Some(sections.yaml_start_line),
                "frontmatter has a key that is not a string",
            ));
        };
        if !KNOWN_KEYS.contains(&name) {
            return Err(ParseError::new(
                "memory-unknown-key",
                key_line(sections, name),
                format!(
                    "`{name}` is not a frontmatter key of this format; the keys are {known}",
                    known = KNOWN_KEYS.join(", ")
                ),
            ));
        }
    }
    Ok(())
}

fn parse_validations(
    raw: Vec<RawValidated>,
    sections: &Sections<'_>,
) -> std::result::Result<Vec<Validated>, ParseError> {
    let mut validated = Vec::with_capacity(raw.len());
    for (position, entry) in raw.into_iter().enumerate() {
        let at_time = DateTime::parse_from_rfc3339(&entry.at)
            .map_err(|time_error| {
                ParseError::new(
                    "memory-frontmatter",
                    key_line(sections, "validated"),
                    format!(
                        "`validated[{position}].at` is {at:?}, not an RFC 3339 timestamp: \
                         {time_error}",
                        at = entry.at,
                    ),
                )
            })?
            .with_timezone(&Utc);
        validated.push(Validated {
            at: entry.at,
            by: entry.by,
            how: entry.how,
            ok: entry.ok,
            at_time,
        });
    }
    Ok(validated)
}

fn parse_based_on(
    raw: Vec<RawBasedOn>,
    sections: &Sections<'_>,
) -> std::result::Result<Vec<BasedOn>, ParseError> {
    let mut based_on = Vec::with_capacity(raw.len());
    for entry in raw {
        // A hash that is not hex cannot be compared with a computed one, and a
        // silent skip there would report a stale memory as current.
        if let Some(hash) = &entry.blake3
            && !is_lowercase_hex(hash)
        {
            return Err(ParseError::new(
                "memory-frontmatter",
                key_line(sections, "based_on"),
                format!("`based_on` hash {hash:?} is not lowercase hex"),
            ));
        }
        based_on.push(BasedOn {
            path: entry.path,
            blake3: entry.blake3,
        });
    }
    Ok(based_on)
}

/// Read and parse one memory file.
///
/// # Errors
///
/// Returns [`crate::Error::ReadFile`] if the file cannot be read and
/// [`crate::Error::Malformed`] if it does not parse.
pub fn load_memory(path: &Path, root: &Path) -> Result<Memory> {
    let contents = std::fs::read_to_string(path).context(error::ReadFileSnafu { path })?;
    parse_memory(path, root, &contents).map_err(|parse_error| error::Error::Malformed {
        path: path.to_path_buf(),
        rule: parse_error.rule,
        message: parse_error.message,
    })
}

/// Translate a `serde_norway` error location, which is 1-based inside the YAML
/// block, into a file line.
fn yaml_line(yaml_error: &serde_norway::Error, yaml_start_line: usize) -> Option<usize> {
    yaml_error
        .location()
        .map(|location| location.line() + yaml_start_line - 1)
}

/// File line of a top-level frontmatter key, for a diagnostic that points at
/// the offending line rather than the top of the block.
fn key_line(sections: &Sections<'_>, key: &str) -> Option<usize> {
    let prefix = format!("{key}:");
    sections
        .yaml
        .lines()
        .position(|line| line.starts_with(&prefix))
        .map(|offset| offset + sections.yaml_start_line)
}

fn is_lowercase_hex(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// A slug is kebab-case: lowercase letters, digits, and single interior
/// hyphens. Anything else makes the file stem, which is the identity, differ
/// from what a `related:` reference would naturally spell.
#[must_use]
pub fn is_kebab_case(slug: &str) -> bool {
    !slug.is_empty()
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && !slug.contains("--")
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Format an instant the way the format writes `validated.at`: RFC 3339, UTC,
/// second resolution.
#[must_use]
pub fn format_timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Render `value` as a YAML scalar, quoting only when it has to be quoted.
///
/// A plain scalar is quoted when it would be read back as something else (a
/// number, a boolean, a nested mapping). The alternative, quoting everything, is
/// safe but makes every `tldr` in every diff read as a quoted blob; this keeps
/// the common case plain and pays for it with the round-trip test in this
/// module.
#[must_use]
pub fn yaml_scalar(value: &str) -> String {
    if needs_quoting(value) {
        quote(value)
    } else {
        value.to_owned()
    }
}

/// Render `value` as a YAML scalar inside a flow sequence (`[a, b]`), where the
/// separators are part of the syntax and a value containing one has to be
/// quoted even though it would be a fine block scalar.
#[must_use]
pub fn yaml_flow_scalar(value: &str) -> String {
    if value.contains([',', '[', ']', '{', '}']) {
        quote(value)
    } else {
        yaml_scalar(value)
    }
}

fn quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// YAML indicator characters that change a scalar's meaning when they lead it.
const LEADING_INDICATORS: [char; 15] = [
    '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '%',
];

fn needs_quoting(value: &str) -> bool {
    if value.is_empty() || value.trim() != value {
        return true;
    }
    if value.starts_with(LEADING_INDICATORS) || value.starts_with('\'') || value.starts_with('"') {
        return true;
    }
    if value.contains(": ") || value.ends_with(':') || value.contains(" #") {
        return true;
    }
    if value.contains(['\n', '\r', '\t']) {
        return true;
    }
    // A plain scalar that reads as a number, boolean, or null would come back
    // as that type rather than as a string.
    if value.parse::<f64>().is_ok() || value.parse::<i64>().is_ok() {
        return true;
    }
    matches!(
        value.to_ascii_lowercase().as_str(),
        "true" | "false" | "yes" | "no" | "on" | "off" | "null" | "~"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(contents: &str) -> std::result::Result<Memory, ParseError> {
        parse_memory(
            Path::new("/repo/.memories/nix-rebuild-cascade.md"),
            Path::new("/repo"),
            contents,
        )
    }

    #[test]
    fn minimal_memory_parses_with_documented_defaults() {
        let memory = parse("---\ntldr: An env var holding a store path\n---\nBody.\n")
            .expect("a `tldr` is all the format requires");
        assert_eq!(memory.slug, "nix-rebuild-cascade");
        assert_eq!(memory.genre, Genre::Memory, "genre defaults to memory");
        assert!(
            (memory.prior - DEFAULT_PRIOR).abs() < f64::EPSILON,
            "prior defaults to {DEFAULT_PRIOR}, got {}",
            memory.prior
        );
        assert_eq!(memory.body, "Body.\n");
        assert_eq!(memory.scope, Scope::Shared, "scope defaults to shared");
    }

    #[test]
    fn no_fence_is_a_frontmatter_diagnostic() {
        let error = parse("Just a body, no frontmatter.\n").expect_err("must not be skipped");
        assert_eq!(error.rule, "memory-frontmatter");
        assert_eq!(error.line, Some(1));
        assert!(error.message.contains("no frontmatter"), "{error:?}");
    }

    #[test]
    fn unterminated_fence_is_its_own_diagnostic() {
        let error = parse("---\ntldr: Something\nstill in the block\n").expect_err("unterminated");
        assert_eq!(error.rule, "memory-frontmatter");
        assert!(
            error.message.contains("unterminated"),
            "the fix differs from the no-fence case, so the message must too: {error:?}"
        );
    }

    #[test]
    fn empty_frontmatter_is_its_own_diagnostic() {
        let error = parse("---\n---\nBody.\n").expect_err("empty frontmatter");
        assert_eq!(error.rule, "memory-frontmatter");
        assert!(error.message.contains("empty"), "{error:?}");
    }

    #[test]
    fn missing_tldr_is_a_tldr_diagnostic_not_a_frontmatter_one() {
        let error = parse("---\ngenre: recipe\n---\nBody.\n").expect_err("tldr is required");
        assert_eq!(error.rule, "memory-tldr");
        assert!(error.message.contains("tldr"), "{error:?}");
    }

    #[test]
    fn scope_parses_both_forms_and_rejects_anything_else() {
        let shared = parse("---\ntldr: A line\nscope: shared\n---\nBody.\n")
            .expect("`shared` is the default form");
        assert_eq!(shared.scope, Scope::Shared);

        let mine = parse("---\ntldr: A line\nscope: user:andrewgazelka\n---\nBody.\n")
            .expect("`user:<name>` is the other form");
        assert_eq!(mine.scope, Scope::User("andrewgazelka".to_owned()));
        assert_eq!(mine.scope.rendered(), "user:andrewgazelka");

        let error = parse("---\ntldr: A line\nscope: everyone\n---\nBody.\n")
            .expect_err("`scope` is a closed form");
        assert_eq!(error.rule, "memory-frontmatter");
        assert_eq!(error.line, Some(3), "must point at the `scope` line");
    }

    /// `always:` was retired deliberately: nothing is injected unasked, so a
    /// memory reaches a model only because something searched for it. The
    /// unknown-key rule is what stops it coming back by habit.
    #[test]
    fn a_retired_key_is_its_own_rule() {
        let error = parse("---\ntldr: A line\nalways: true\n---\nBody.\n")
            .expect_err("`always` is not a key of this format");
        assert_eq!(error.rule, "memory-unknown-key");
        assert_eq!(error.line, Some(3));
        assert!(error.message.contains("always"), "{error:?}");
    }

    #[test]
    fn unknown_key_is_an_error_rather_than_an_ignored_line() {
        // `topics` for `topic` is the mistake this rejects: a silently ignored
        // key is a memory that never matches the topic filter its author set.
        let error = parse("---\ntldr: A line\ntopics: [nix]\n---\nBody.\n")
            .expect_err("an unknown key must not be dropped");
        assert_eq!(error.rule, "memory-unknown-key");
        assert!(error.message.contains("topics"), "{error:?}");
    }

    #[test]
    fn slug_in_frontmatter_parses_but_is_recorded_for_the_linter() {
        let memory = parse("---\ntldr: A line\nslug: something-else\n---\nBody.\n")
            .expect("the linter, not the parser, reports a written slug");
        assert_eq!(memory.frontmatter_slug.as_deref(), Some("something-else"));
        assert_eq!(memory.slug, "nix-rebuild-cascade", "slug is the file stem");
    }

    #[test]
    fn unknown_genre_is_an_error_with_a_line() {
        let error = parse("---\ntldr: A line\ngenre: folklore\n---\nBody.\n")
            .expect_err("genre is a closed set");
        assert_eq!(error.rule, "memory-frontmatter");
        assert!(error.line.is_some(), "expected a line number: {error:?}");
    }

    #[test]
    fn out_of_range_prior_is_an_error_rather_than_clamped() {
        let error =
            parse("---\ntldr: A line\nprior: 1.4\n---\nBody.\n").expect_err("prior is 0..1");
        assert_eq!(error.rule, "memory-frontmatter");
        assert_eq!(error.line, Some(3), "must point at the `prior` line");
    }

    #[test]
    fn unparseable_validated_at_is_an_error() {
        let contents = concat!(
            "---\n",
            "tldr: A line\n",
            "validated:\n",
            "  - at: last tuesday\n",
            "    by: someone\n",
            "    how: a command\n",
            "    ok: true\n",
            "---\n",
            "Body.\n",
        );
        let error = parse(contents).expect_err("`at` must be RFC 3339");
        assert_eq!(error.rule, "memory-frontmatter");
        assert!(error.message.contains("RFC 3339"), "{error:?}");
    }

    #[test]
    fn newest_validation_decides_refutation_by_time_not_file_order() {
        let contents = concat!(
            "---\n",
            "tldr: A line\n",
            "validated:\n",
            "  - at: 2026-07-01T00:00:00Z\n",
            "    by: a\n",
            "    how: c1\n",
            "    ok: false\n",
            "  - at: 2026-06-01T00:00:00Z\n",
            "    by: b\n",
            "    how: c2\n",
            "    ok: true\n",
            "---\n",
            "Body.\n",
        );
        let memory = parse(contents).expect("two validations");
        assert!(
            memory.is_refuted(),
            "the July entry is newest even though June is written last"
        );
        assert_eq!(memory.ok_count(), 1);
    }

    #[test]
    fn body_is_preserved_byte_for_byte_including_fences_inside_it() {
        let contents = "---\ntldr: A line\n---\nBody with a --- inside\n\nand a blank line.\n";
        let memory = parse(contents).expect("valid");
        assert_eq!(memory.body, "Body with a --- inside\n\nand a blank line.\n");
    }

    #[test]
    fn kebab_case_check_rejects_the_shapes_a_slug_cannot_take() {
        assert!(is_kebab_case("nix-rebuild-cascade"));
        assert!(is_kebab_case("ix2nix-2"));
        assert!(!is_kebab_case("Nix-Rebuild"));
        assert!(!is_kebab_case("nix_rebuild"));
        assert!(!is_kebab_case("-leading"));
        assert!(!is_kebab_case("trailing-"));
        assert!(!is_kebab_case("double--hyphen"));
        assert!(!is_kebab_case(""));
    }

    /// Every scalar the writers emit has to come back out of the YAML parser
    /// unchanged, including the ones that look like other types.
    #[test]
    fn yaml_scalars_round_trip_through_the_parser() {
        let nasty = [
            "plain words",
            "it does not block: it launches more work",
            "trailing colon:",
            "#not a comment",
            "- not a list",
            "true",
            "42",
            "1.5",
            "null",
            "~",
            "with \"quotes\" inside",
            "with 'single' quotes",
            "back\\slash",
            "  leading and trailing  ",
            "a # hash",
            "{braces}",
            "[brackets]",
            "",
        ];
        for value in nasty {
            let document = format!("tldr: {}\n", yaml_scalar(value));
            let parsed: std::collections::BTreeMap<String, String> =
                serde_norway::from_str(&document)
                    .unwrap_or_else(|error| panic!("{document:?} must parse: {error}"));
            assert_eq!(
                parsed.get("tldr").map(String::as_str),
                Some(value),
                "round trip failed for {value:?} rendered as {document:?}"
            );
        }
    }
}
