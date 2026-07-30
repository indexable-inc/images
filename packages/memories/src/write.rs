//! The only writer of the on-disk format.
//!
//! Nothing else renders frontmatter, so the format has one writer and cannot
//! drift between callers.
//!
//! `validate`, `refute` and `lint --fix` edit the existing text rather than
//! reserializing a parsed document: a round trip through a YAML emitter would
//! restyle every line it did not mean to touch, and a memory's history is the
//! part a reader trusts. Everything outside the lines being changed comes back
//! byte for byte.

use crate::{
    discover::Root,
    error::{self, Result},
    model::{self, BasedOn, Genre, Memory, Scope, Validated},
    stale,
};
use snafu::ResultExt;
use std::path::{Path, PathBuf};

/// What `remember` writes. Every field is what the caller passed; nothing here
/// is inferred except the `based_on` hashes, which are computed now.
#[derive(Debug)]
pub struct RememberSpec<'a> {
    pub slug: &'a str,
    pub tldr: &'a str,
    pub genre: Genre,
    pub topic: &'a [String],
    pub handle: &'a [String],
    pub prior: f64,
    pub related: &'a [String],
    pub based_on: &'a [String],
    pub scope: Scope,
    /// The proof that the memory held, written at birth.
    ///
    /// Required for `genre: memory` by the CLI, because you write a memory at the
    /// moment you learn something, which is exactly the moment you still have the
    /// command that proved it. Making that a second step would put the honest
    /// path at two commands and the lazy one at one.
    pub first_validation: Option<&'a Validated>,
    pub body: &'a str,
}

/// Write a new memory into `root`, creating the `.memories` directory if it is
/// not there yet.
///
/// # Errors
///
/// Returns [`crate::Error::SlugExists`] rather than overwriting: a `remember`
/// over an existing slug would drop that memory's `validated` history, which is
/// the one part of a memory that cannot be reconstructed. Returns
/// [`crate::Error::BasedOnMissing`] when a `--based-on` path matches nothing,
/// so a memory is never born failing its own lint.
pub fn remember(root: &Root, spec: &RememberSpec<'_>) -> Result<PathBuf> {
    std::fs::create_dir_all(&root.memories_dir).context(error::CreateDirSnafu {
        path: &root.memories_dir,
    })?;

    let path = root
        .memories_dir
        .join(format!("{slug}.md", slug = spec.slug));
    if path.exists() {
        return error::SlugExistsSnafu {
            slug: spec.slug.to_owned(),
            path,
        }
        .fail();
    }

    // Built as lines and joined once: the frontmatter is written in the
    // format's key order, and a line list keeps that order readable.
    let mut lines: Vec<String> = vec![
        "---".to_owned(),
        format!("tldr: {}", model::yaml_scalar(spec.tldr.trim())),
        format!("genre: {}", genre_name(spec.genre)),
    ];
    push_flow_list(&mut lines, "topic", spec.topic);
    push_flow_list(&mut lines, "handle", spec.handle);
    lines.push(format!("prior: {}", format_prior(spec.prior)));
    push_flow_list(&mut lines, "related", spec.related);

    if !spec.based_on.is_empty() {
        lines.push("based_on:".to_owned());
        for target in spec.based_on {
            let entry = BasedOn {
                path: target.clone(),
                blake3: None,
            };
            if !stale::resolves(&root.root, &entry)? {
                return error::BasedOnMissingSnafu {
                    path: target.clone(),
                    root: root.root.clone(),
                }
                .fail();
            }
            lines.push(format!("  - path: {}", model::yaml_scalar(target)));
            // A glob stands for a set of files, so there is no single content
            // to hash and the format omits the key.
            if !entry.is_glob()
                && let Some(hash) = stale::hash_file(&root.root.join(target))?
            {
                lines.push(format!("    blake3: {hash}"));
            }
        }
    }

    if let Some(entry) = spec.first_validation {
        lines.push("validated:".to_owned());
        // The rendered entry carries its own line breaks, so push it as a block
        // and let the join below leave it alone.
        lines.push(
            render_validation(entry, "\n")
                .trim_end_matches('\n')
                .to_owned(),
        );
    }

    // `scope` is only written when it is not the default: an absent key and
    // `shared` mean the same thing, and the shorter file is the readable one.
    if spec.scope != Scope::Shared {
        lines.push(format!(
            "scope: {}",
            model::yaml_scalar(&spec.scope.rendered())
        ));
    }
    lines.push("---".to_owned());

    let frontmatter = lines.join("\n") + "\n";
    let body = spec.body.trim_start_matches('\n');
    let contents = if body.trim().is_empty() {
        frontmatter
    } else if body.ends_with('\n') {
        format!("{frontmatter}{body}")
    } else {
        format!("{frontmatter}{body}\n")
    };

    std::fs::write(&path, contents).context(error::WriteFileSnafu { path: &path })?;
    Ok(path)
}

/// Append a `validated` entry to an existing memory and refresh every
/// `based_on` hash in the same write, so validating clears staleness.
///
/// # Errors
///
/// Returns [`crate::Error::UnwritableFrontmatter`] when the file's frontmatter
/// does not parse, the same refusal `skill-lint --fix` makes: a document we
/// cannot read is a document we must not rewrite.
pub fn append_validation(memory: &Memory, entry: &Validated) -> Result<Vec<String>> {
    let contents = read(&memory.path)?;
    let mut file = split_file(&contents).ok_or_else(|| error::Error::UnwritableFrontmatter {
        path: memory.path.clone(),
        message: "no `---` … `---` block".to_owned(),
    })?;

    let mut notes = refresh_hashes(&mut file, memory)?;
    insert_validation(&mut file, entry);
    notes.push(format!(
        "appended a `validated` entry dated {at}",
        at = entry.at
    ));

    write(&memory.path, &file.render())?;
    Ok(notes)
}

/// Add `slug` to a memory's `supersedes` list, for `refute --instead`. The
/// successor is where `supersedes` lives in the format, so refuting one memory
/// in favour of another writes to both files.
///
/// # Errors
///
/// Returns [`crate::Error::UnwritableFrontmatter`] when the successor's
/// frontmatter does not parse.
pub fn add_supersedes(memory: &Memory, slug: &str) -> Result<Vec<String>> {
    if memory.supersedes.iter().any(|existing| existing == slug) {
        return Ok(Vec::new());
    }

    let contents = read(&memory.path)?;
    let mut file = split_file(&contents).ok_or_else(|| error::Error::UnwritableFrontmatter {
        path: memory.path.clone(),
        message: "no `---` … `---` block".to_owned(),
    })?;

    let mut supersedes = memory.supersedes.clone();
    supersedes.push(slug.to_owned());
    replace_key_with_flow_list(&mut file, "supersedes", &supersedes);

    write(&memory.path, &file.render())?;
    Ok(vec![format!(
        "added {slug} to {other}'s `supersedes`",
        other = memory.slug
    )])
}

/// Apply the unambiguous fixes: sort `topic` and `handle`, refresh `based_on`
/// hashes, normalize whitespace. Returns one line per change, empty when the
/// file was already clean.
///
/// # Errors
///
/// Returns [`crate::Error::UnwritableFrontmatter`] when the frontmatter does not
/// parse, and an IO error when the file cannot be read or written.
pub fn fix(memory: &Memory) -> Result<Vec<String>> {
    let contents = read(&memory.path)?;
    let mut file = split_file(&contents).ok_or_else(|| error::Error::UnwritableFrontmatter {
        path: memory.path.clone(),
        message: "no `---` … `---` block".to_owned(),
    })?;

    let mut notes = Vec::new();

    for (key, values) in [("topic", &memory.topic), ("handle", &memory.handle)] {
        let mut sorted = values.clone();
        sorted.sort();
        // Only touch an out-of-order list. Rewriting a sorted one would restyle
        // a block list into a flow list for no reason.
        if sorted != *values {
            replace_key_with_flow_list(&mut file, key, &sorted);
            notes.push(format!("sorted `{key}`"));
        }
    }

    notes.extend(refresh_hashes(&mut file, memory)?);

    let edited = file.render();
    let normalized = normalize_whitespace(&edited, file.newline);
    if normalized != edited {
        notes.push("normalized whitespace".to_owned());
    }
    if normalized != contents {
        write(&memory.path, &normalized)?;
    }
    Ok(notes)
}

/// Build the `validated` entry a `validate` or `refute` call records.
#[must_use]
pub fn validation(by: &str, how: &str, ok: bool, at: chrono::DateTime<chrono::Utc>) -> Validated {
    Validated {
        at: model::format_timestamp(at),
        by: by.to_owned(),
        how: how.to_owned(),
        ok,
        at_time: at,
    }
}

const fn genre_name(genre: Genre) -> &'static str {
    match genre {
        Genre::Memory => "memory",
        Genre::Living => "living",
        Genre::Recipe => "recipe",
        Genre::Historical => "historical",
        Genre::Frozen => "frozen",
    }
}

/// `prior` with a decimal point even when it is whole, so `1` cannot be read
/// back as an integer by a stricter parser than the one that wrote it.
fn format_prior(prior: f64) -> String {
    if (prior.fract()).abs() < f64::EPSILON {
        format!("{prior:.1}")
    } else {
        format!("{prior}")
    }
}

fn push_flow_list(lines: &mut Vec<String>, key: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    lines.push(flow_list_line(key, values));
}

fn flow_list_line(key: &str, values: &[String]) -> String {
    let rendered: Vec<String> = values
        .iter()
        .map(|value| model::yaml_flow_scalar(value))
        .collect();
    format!("{key}: [{}]", rendered.join(", "))
}

/// A memory file split into its exact pieces. `render` is the identity for a
/// file nothing has touched.
#[derive(Debug)]
struct FrontmatterFile {
    /// Opening fence line, with its terminator.
    open: String,
    /// YAML block, exactly as written.
    yaml: String,
    /// Closing fence line, with its terminator.
    close: String,
    /// Everything after the closing fence.
    body: String,
    /// The file's dominant line ending, for lines this module adds.
    newline: &'static str,
}

impl FrontmatterFile {
    fn render(&self) -> String {
        format!(
            "{open}{yaml}{close}{body}",
            open = self.open,
            yaml = self.yaml,
            close = self.close,
            body = self.body
        )
    }
}

fn split_file(contents: &str) -> Option<FrontmatterFile> {
    if contents.lines().next()?.trim_end() != "---" {
        return None;
    }
    let open_len = contents.find('\n')? + 1;
    let rest = &contents[open_len..];

    let mut offset = 0usize;
    for chunk in rest.split_inclusive('\n') {
        if chunk.trim_end() == "---" {
            return Some(FrontmatterFile {
                open: contents[..open_len].to_owned(),
                yaml: rest[..offset].to_owned(),
                close: chunk.to_owned(),
                body: rest[offset + chunk.len()..].to_owned(),
                // A CRLF file rewritten with LF endings is a whole-file diff,
                // so added lines take the ending the file already uses.
                newline: if contents.contains("\r\n") {
                    "\r\n"
                } else {
                    "\n"
                },
            });
        }
        offset += chunk.len();
    }
    None
}

/// Insert a `validated` entry after the last line of the existing block, or
/// append the block when there is none. Appending rather than replacing is the
/// whole point: the history is the evidence.
fn insert_validation(file: &mut FrontmatterFile, entry: &Validated) {
    let newline = file.newline;
    let rendered = render_validation(entry, newline);

    let lines = split_lines(&file.yaml);
    if let Some(block) = key_block(&lines, "validated") {
        let mut yaml = String::with_capacity(file.yaml.len() + rendered.len());
        for (index, line) in lines.iter().enumerate() {
            yaml.push_str(line);
            if index + 1 == block.end {
                yaml.push_str(&rendered);
            }
        }
        file.yaml = yaml;
    } else {
        if !file.yaml.is_empty() && !file.yaml.ends_with('\n') {
            file.yaml.push_str(newline);
        }
        file.yaml.push_str("validated:");
        file.yaml.push_str(newline);
        file.yaml.push_str(&rendered);
    }
}

/// One `validated:` sequence entry, indented the way the format writes it. One
/// function so a birth entry and an appended one cannot drift apart.
fn render_validation(entry: &Validated, newline: &str) -> String {
    format!(
        "  - at: {at}{newline}    by: {by}{newline}    how: {how}{newline}    ok: {ok}{newline}",
        at = model::yaml_scalar(&entry.at),
        by = model::yaml_scalar(&entry.by),
        how = model::yaml_scalar(&entry.how),
        ok = entry.ok,
    )
}

/// Recompute every `based_on` hash and write the current value, adding the key
/// where an entry has none.
///
/// Returns a note per entry this could not refresh: one whose file has moved
/// (the recorded hash is left alone, because there is no current content to
/// replace it with and dropping the key would erase the evidence the memory ever
/// had one), and one per entry whose lines this walker did not recognize.
fn refresh_hashes(file: &mut FrontmatterFile, memory: &Memory) -> Result<Vec<String>> {
    if memory.based_on.is_empty() {
        return Ok(Vec::new());
    }

    let mut notes = Vec::new();
    let lines = split_lines(&file.yaml);
    let Some(block) = key_block(&lines, "based_on") else {
        notes.push(format!(
            "based_on has {count} entries but no `based_on:` block to rewrite, so no hash was \
             refreshed",
            count = memory.based_on.len()
        ));
        return Ok(notes);
    };

    let mut output = String::with_capacity(file.yaml.len());
    let mut current: Option<CurrentEntry> = None;
    // Entries whose `path:` line this walker recognized. The parsed frontmatter
    // says how many there should be, and a shortfall means a shape the walker
    // does not handle (a flow-style sequence, say). That must be said out loud:
    // silently refreshing nothing would report a stale memory as validated.
    let mut rewritten = 0usize;

    for (index, line) in lines.iter().enumerate() {
        if !block.contains(&index) {
            output.push_str(line);
            continue;
        }

        if let Some(path) = path_value(line) {
            let entry = memory
                .based_on
                .iter()
                .find(|candidate| candidate.path == path);
            let hash = match entry {
                // A glob has no single content to hash.
                Some(entry) if entry.is_glob() => None,
                Some(_) => stale::hash_file(&memory.root.join(&path))?,
                None => None,
            };
            if hash.is_none() && entry.is_some_and(|entry| !entry.is_glob()) {
                notes.push(format!(
                    "based_on {path} is missing, so its recorded hash was left as written"
                ));
            }
            output.push_str(line);
            if let Some(hash) = &hash
                && !entry_has_hash(&lines, index, &block)
            {
                output.push_str(&hash_line(line, hash, file.newline));
            }
            current = Some(CurrentEntry { hash });
            rewritten += 1;
            continue;
        }

        if is_hash_line(line) {
            match current.as_ref().and_then(|entry| entry.hash.as_deref()) {
                Some(hash) => output.push_str(&rewrite_hash_line(line, hash)),
                // Unknown path or a moved file: leave the line exactly as it is.
                None => output.push_str(line),
            }
            continue;
        }

        output.push_str(line);
    }

    if rewritten < memory.based_on.len() {
        notes.push(format!(
            "only {rewritten} of {total} `based_on` entries are in a shape this writer can \
             rewrite; the rest keep the hash they had",
            total = memory.based_on.len()
        ));
    }

    file.yaml = output;
    Ok(notes)
}

/// The `based_on` entry currently being rewritten, and the hash its file has
/// now (`None` when the file has moved or the path is a glob).
struct CurrentEntry {
    hash: Option<String>,
}

/// Replace a top-level key's whole block with a single flow-list line, or add
/// the key when it is absent. Restyling a block list to a flow list is a real
/// change to the file, which is why callers only do it when the values changed.
fn replace_key_with_flow_list(file: &mut FrontmatterFile, key: &str, values: &[String]) {
    let newline = file.newline;
    let replacement = format!("{}{newline}", flow_list_line(key, values));
    let lines = split_lines(&file.yaml);

    if let Some(block) = key_block(&lines, key) {
        let mut yaml = String::with_capacity(file.yaml.len() + replacement.len());
        for (index, line) in lines.iter().enumerate() {
            if index == block.start {
                yaml.push_str(&replacement);
            } else if !block.contains(&index) {
                yaml.push_str(line);
            }
        }
        file.yaml = yaml;
    } else {
        if !file.yaml.is_empty() && !file.yaml.ends_with('\n') {
            file.yaml.push_str(newline);
        }
        file.yaml.push_str(&replacement);
    }
}

/// Lines with their terminators, so rebuilding is byte-exact.
fn split_lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// Half-open line range of a top-level key's block: the `key:` line plus every
/// indented line under it. A blank line ends the block, which is what a reader
/// assumes too.
fn key_block(lines: &[&str], key: &str) -> Option<std::ops::Range<usize>> {
    let prefix = format!("{key}:");
    let start = lines
        .iter()
        .position(|line| line.starts_with(&prefix) && !line.starts_with(char::is_whitespace))?;
    let mut end = start + 1;
    while lines
        .get(end)
        .is_some_and(|line| line.starts_with([' ', '\t']))
    {
        end += 1;
    }
    Some(start..end)
}

/// The `path:` value of a `based_on` entry line, in either shape a YAML
/// sequence entry can take (`- path: x` or a bare `path: x` continuation).
fn path_value(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let trimmed = trimmed.strip_prefix("- ").unwrap_or(trimmed);
    let value = trimmed.strip_prefix("path:")?;
    Some(unquote(strip_comment(value).trim()))
}

fn is_hash_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let trimmed = trimmed.strip_prefix("- ").unwrap_or(trimmed);
    trimmed.starts_with("blake3:")
}

/// Whether the `based_on` entry starting at `path_index` already carries a
/// `blake3:` line, scanning only that entry's own lines.
fn entry_has_hash(lines: &[&str], path_index: usize, block: &std::ops::Range<usize>) -> bool {
    for line in lines.iter().take(block.end).skip(path_index + 1) {
        if is_entry_start(line) || path_value(line).is_some() {
            break;
        }
        if is_hash_line(line) {
            return true;
        }
    }
    false
}

fn is_entry_start(line: &str) -> bool {
    line.trim_start().starts_with("- ")
}

/// A `blake3:` line for an entry whose `path:` line is `path_line`, indented to
/// match. A `- path:` line's continuation keys line up under the `path`, two
/// columns right of the dash.
fn hash_line(path_line: &str, hash: &str, newline: &str) -> String {
    let indent = leading_whitespace(path_line);
    let dash_width = if path_line.trim_start().starts_with("- ") {
        2
    } else {
        0
    };
    format!("{indent}{}blake3: {hash}{newline}", " ".repeat(dash_width))
}

/// Rewrite a `blake3:` line's value, keeping its indentation, any `- ` prefix,
/// and any trailing comment. The format's own example carries a comment on that
/// line, so dropping it would rewrite documentation the author put there.
fn rewrite_hash_line(line: &str, hash: &str) -> String {
    let indent = leading_whitespace(line);
    let rest = &line[indent.len()..];
    let dash = if rest.starts_with("- ") { "- " } else { "" };
    let terminator = terminator(line);
    let comment = comment_of(line);
    format!("{indent}{dash}blake3: {hash}{comment}{terminator}")
}

fn leading_whitespace(line: &str) -> &str {
    let end = line
        .find(|character: char| !character.is_whitespace())
        .unwrap_or(line.len());
    &line[..end]
}

fn terminator(line: &str) -> &str {
    if line.ends_with("\r\n") {
        "\r\n"
    } else if line.ends_with('\n') {
        "\n"
    } else {
        ""
    }
}

/// The ` # comment` tail of a line, if any, preserved verbatim.
fn comment_of(line: &str) -> String {
    line.trim_end_matches(['\n', '\r'])
        .find(" #")
        .map_or_else(String::new, |at| {
            line[at..].trim_end_matches(['\n', '\r']).to_owned()
        })
}

fn strip_comment(value: &str) -> &str {
    value.find(" #").map_or(value, |at| &value[..at])
}

/// Drop the quotes a YAML scalar may carry, so a quoted `path:` compares equal
/// to the parsed value. Escapes are not interpreted: a path needing them is not
/// a path this format can write.
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        return value[1..value.len() - 1].to_owned();
    }
    value.to_owned()
}

/// Strip trailing whitespace from every line and end the file with exactly one
/// newline, preserving the file's dominant line ending. `str::lines` drops
/// `\r`, so the rejoin uses the detected ending rather than a hardcoded `\n`.
fn normalize_whitespace(contents: &str, newline: &str) -> String {
    let mut lines: Vec<&str> = contents.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return String::new();
    }
    let mut out = lines.join(newline);
    out.push_str(newline);
    out
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).context(error::ReadFileSnafu { path })
}

fn write(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents).context(error::WriteFileSnafu { path })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        fixture::{Repo, fixed_now},
        lint,
    };

    fn spec<'a>(slug: &'a str, tldr: &'a str) -> RememberSpec<'a> {
        RememberSpec {
            slug,
            tldr,
            genre: Genre::Memory,
            topic: &[],
            handle: &[],
            prior: model::DEFAULT_PRIOR,
            related: &[],
            based_on: &[],
            scope: Scope::Shared,
            first_validation: None,
            body: "Why, the evidence, the exact command.\n",
        }
    }

    fn only_memory(repo: &Repo) -> Memory {
        let corpus = repo.load();
        assert!(
            corpus.failures.is_empty(),
            "fixture must parse: {:?}",
            corpus.failures
        );
        corpus
            .memories
            .into_iter()
            .next()
            .expect("one memory in the fixture")
    }

    #[test]
    fn remember_round_trips_through_the_parser_and_passes_lint() {
        let repo = Repo::new();
        repo.file("src/rank.rs", "fn main() {}\n");
        let topic = vec!["nix".to_owned(), "builds".to_owned()];
        let handle = vec!["nix-dag".to_owned()];
        let related = Vec::new();
        let based_on = vec!["src/rank.rs".to_owned()];
        let mut written = spec("nix-rebuild-cascade", "it does not block: it launches work");
        written.topic = &topic;
        written.handle = &handle;
        written.related = &related;
        written.based_on = &based_on;
        written.prior = 0.8;

        let path = remember(&repo.roots()[0], &written).expect("writing a new memory");
        assert_eq!(path, repo.memory_path("nix-rebuild-cascade"));

        let memory = only_memory(&repo);
        assert_eq!(memory.tldr, "it does not block: it launches work");
        assert_eq!(memory.topic, topic);
        assert_eq!(memory.handle, handle);
        assert!((memory.prior - 0.8).abs() < f64::EPSILON);
        assert_eq!(memory.based_on.len(), 1);
        assert!(
            memory.based_on[0].blake3.is_some(),
            "remember records the hash it saw"
        );
        assert_eq!(memory.body, "Why, the evidence, the exact command.\n");

        // Without a birth validation this is a `genre: memory` nobody has proved,
        // which is what `memory-unchecked` is for. The CLI requires `--by`/`--how`
        // so that state is not reachable through it; the library still allows it,
        // because a `living` page legitimately has no proof.
        let diagnostics = lint::lint(&repo.load(), fixed_now()).expect("linting");
        assert_eq!(
            diagnostics.iter().map(|d| d.rule).collect::<Vec<_>>(),
            ["memory-unchecked"],
            "got {diagnostics:?}"
        );
    }

    /// The gap dogfooding found: a `remember` that needs a second command to make
    /// its own file legal puts the honest path at two steps and the lazy one at
    /// one. The proof is written at birth, so `remember` then `lint` is clean.
    #[test]
    fn remember_writes_its_first_validation_and_lints_clean_in_one_step() {
        let repo = Repo::new();
        let entry = validation(
            "claude-opus-5",
            "nix-dag .#hil-compute-2; top sole-count node was IX_ASSETS_DIR",
            true,
            fixed_now(),
        );
        let mut written = spec("nix-rebuild-cascade", "An env var makes every host rebuild");
        written.first_validation = Some(&entry);
        remember(&repo.roots()[0], &written).expect("writing a new memory");

        let memory = only_memory(&repo);
        assert_eq!(memory.validated.len(), 1, "the proof rides along");
        assert_eq!(memory.ok_count(), 1, "a memory you just learned held");
        assert_eq!(memory.validated[0].by, "claude-opus-5");
        assert_eq!(
            memory.validated[0].how,
            "nix-dag .#hil-compute-2; top sole-count node was IX_ASSETS_DIR",
            "the command survives its `;` and `#` unquoted-YAML hazards"
        );

        let diagnostics = lint::lint(&repo.load(), fixed_now()).expect("linting");
        assert!(
            diagnostics.is_empty(),
            "one command must produce a legal memory: {diagnostics:?}"
        );
    }

    #[test]
    fn remember_refuses_to_overwrite_an_existing_slug() {
        let repo = Repo::new();
        repo.memory("taken", "validated_today", "Body.\n");
        let error = remember(&repo.roots()[0], &spec("taken", "A new line"))
            .expect_err("overwriting would drop the validation history");
        assert!(error.to_string().contains("already exists"), "{error}");
    }

    #[test]
    fn remember_refuses_a_based_on_path_that_does_not_exist() {
        let repo = Repo::new();
        let based_on = vec!["src/gone.rs".to_owned()];
        let mut written = spec("a-slug", "A line");
        written.based_on = &based_on;
        let error = remember(&repo.roots()[0], &written)
            .expect_err("a memory must not be born failing its own lint");
        assert!(error.to_string().contains("src/gone.rs"), "{error}");
    }

    #[test]
    fn validate_appends_and_leaves_every_other_byte_alone() {
        let repo = Repo::new();
        let original = concat!(
            "---\n",
            "tldr: An env var holding a store path\n",
            "genre: memory\n",
            "topic: [nix, builds]\n",
            "validated:\n",
            "  - at: 2026-01-01T00:00:00Z\n",
            "    by: someone\n",
            "    how: \"the first command\"\n",
            "    ok: true\n",
            "scope: shared\n",
            "---\n",
            "Body with   odd    spacing kept as written.\n",
        );
        repo.raw("a-slug.md", original);

        let memory = only_memory(&repo);
        let entry = validation(
            "claude-opus-5",
            "nix-dag .#hil-compute-2",
            true,
            fixed_now(),
        );
        append_validation(&memory, &entry).expect("appending a validation");

        let after = std::fs::read_to_string(repo.memory_path("a-slug")).expect("reading back");
        assert!(
            after.starts_with("---\ntldr: An env var holding a store path\ngenre: memory\n"),
            "prefix rewritten:\n{after}"
        );
        assert!(
            after.contains("  - at: 2026-01-01T00:00:00Z\n    by: someone\n    how: \"the first command\"\n    ok: true\n"),
            "the existing entry must survive byte for byte:\n{after}"
        );
        assert!(
            after.contains("scope: shared\n---\nBody with   odd    spacing kept as written.\n"),
            "everything after the block must be untouched:\n{after}"
        );

        let reloaded = only_memory(&repo);
        assert_eq!(reloaded.validated.len(), 2, "appended, not replaced");
        assert_eq!(reloaded.validated[1].by, "claude-opus-5");
        assert_eq!(reloaded.ok_count(), 2);
    }

    #[test]
    fn validate_adds_the_block_when_there_is_none() {
        let repo = Repo::new();
        repo.memory("never-validated", "", "Body.\n");
        let memory = only_memory(&repo);
        append_validation(
            &memory,
            &validation("tester", "the command", true, fixed_now()),
        )
        .expect("appending the first validation");

        let reloaded = only_memory(&repo);
        assert_eq!(reloaded.validated.len(), 1);
        assert_eq!(reloaded.body, "Body.\n", "the body is untouched");
    }

    #[test]
    fn validate_refreshes_every_based_on_hash_in_the_same_write() {
        let repo = Repo::new();
        repo.file("src/rank.rs", "fn main() {}\n");
        repo.file("src/other.rs", "fn other() {}\n");
        let stale_hash = "0000000000000000";
        repo.raw(
            "a-slug.md",
            &format!(
                concat!(
                    "---\n",
                    "tldr: A line\n",
                    "based_on:\n",
                    "  - path: src/rank.rs\n",
                    "    blake3: {stale}            # content when last validated\n",
                    "  - path: src/other.rs\n",
                    "---\n",
                    "Body.\n",
                ),
                stale = stale_hash
            ),
        );

        let memory = only_memory(&repo);
        assert!(
            crate::stale::check(&memory).expect("checking").stale,
            "the fixture starts stale"
        );

        append_validation(
            &memory,
            &validation("tester", "the command", true, fixed_now()),
        )
        .expect("validating");

        let after = std::fs::read_to_string(repo.memory_path("a-slug")).expect("reading back");
        assert!(
            after.contains("# content when last validated"),
            "the trailing comment must survive:\n{after}"
        );
        let reloaded = only_memory(&repo);
        assert!(
            !crate::stale::check(&reloaded).expect("checking").stale,
            "validating clears staleness:\n{after}"
        );
        assert!(
            reloaded.based_on[1].blake3.is_some(),
            "an entry with no hash gets one:\n{after}"
        );
    }

    #[test]
    fn validate_keeps_a_recorded_hash_when_the_file_has_moved() {
        let repo = Repo::new();
        repo.raw(
            "a-slug.md",
            "---\ntldr: A line\nbased_on:\n  - path: src/gone.rs\n    blake3: abcdef0123456789\n---\nBody.\n",
        );
        let memory = only_memory(&repo);
        let notes = append_validation(
            &memory,
            &validation("tester", "the command", true, fixed_now()),
        )
        .expect("validating");

        let after = std::fs::read_to_string(repo.memory_path("a-slug")).expect("reading back");
        assert!(
            after.contains("blake3: abcdef0123456789"),
            "there is no current content to write instead:\n{after}"
        );
        assert!(
            notes.iter().any(|note| note.contains("src/gone.rs")),
            "and the caller is told: {notes:?}"
        );
    }

    /// A `based_on` written as a flow sequence parses fine but is not a shape
    /// this line-editing writer can rewrite. Silently refreshing nothing there
    /// would report a stale memory as validated, so it says so.
    #[test]
    fn a_based_on_shape_the_writer_cannot_edit_is_reported() {
        let repo = Repo::new();
        repo.file("src/rank.rs", "fn main() {}\n");
        repo.raw(
            "a-slug.md",
            "---\ntldr: A line\nbased_on: [{path: src/rank.rs}]\n---\nBody.\n",
        );
        let memory = only_memory(&repo);
        assert_eq!(memory.based_on.len(), 1, "the flow sequence still parses");

        let notes = append_validation(
            &memory,
            &validation("tester", "the command", true, fixed_now()),
        )
        .expect("validating");
        assert!(
            notes
                .iter()
                .any(|note| note.contains("shape this writer can rewrite")),
            "the caller must be told the hash was not refreshed: {notes:?}"
        );
    }

    #[test]
    fn refute_writes_supersedes_onto_the_successor() {
        let repo = Repo::new();
        repo.memory("old-lesson", "validated_today", "Body.\n");
        repo.memory("new-lesson", "validated_today", "Body.\n");
        let corpus = repo.load();
        let successor = corpus.by_slug("new-lesson").expect("the successor").clone();

        add_supersedes(&successor, "old-lesson").expect("adding supersedes");
        let reloaded = repo.load();
        assert_eq!(
            reloaded
                .by_slug("new-lesson")
                .expect("the successor")
                .supersedes,
            ["old-lesson"]
        );
    }

    #[test]
    fn fix_sorts_topic_and_handle_and_is_idempotent() {
        let repo = Repo::new();
        repo.raw(
            "a-slug.md",
            "---\ntldr: A line\ntopic: [nix, builds]\nhandle:\n  - zeta\n  - alpha\n---\nBody.\n",
        );
        let notes = fix(&only_memory(&repo)).expect("fixing");
        assert_eq!(notes, ["sorted `topic`", "sorted `handle`"], "{notes:?}");

        let after = std::fs::read_to_string(repo.memory_path("a-slug")).expect("reading back");
        assert!(after.contains("topic: [builds, nix]"), "{after}");
        assert!(after.contains("handle: [alpha, zeta]"), "{after}");
        assert!(after.contains("tldr: A line"), "{after}");
        assert!(after.ends_with("---\nBody.\n"), "{after}");

        let second = fix(&only_memory(&repo)).expect("fixing again");
        assert!(
            second.is_empty(),
            "a second pass must be a no-op: {second:?}"
        );
    }

    #[test]
    fn fix_normalizes_trailing_whitespace_without_touching_a_clean_file() {
        let repo = Repo::new();
        repo.raw("dirty.md", "---\ntldr: A line   \n---\nBody.   \n\n\n");
        let notes = fix(&only_memory(&repo)).expect("fixing");
        assert_eq!(notes, ["normalized whitespace"], "{notes:?}");
        let after = std::fs::read_to_string(repo.memory_path("dirty")).expect("reading back");
        assert_eq!(after, "---\ntldr: A line\n---\nBody.\n");

        let second = fix(&only_memory(&repo)).expect("fixing again");
        assert!(second.is_empty(), "{second:?}");
    }

    #[test]
    fn writers_refuse_a_file_whose_frontmatter_they_cannot_read() {
        // The parse gate means a broken file never reaches the writers through
        // the CLI, so this drives the refusal directly: a document we cannot
        // read is a document we must not rewrite.
        let repo = Repo::new();
        repo.memory("a-slug", "validated_today", "Body.\n");
        let memory = only_memory(&repo);
        std::fs::write(&memory.path, "no frontmatter at all\n").expect("clobbering the file");

        let error = fix(&memory).expect_err("must refuse");
        assert!(error.to_string().contains("Refusing to rewrite"), "{error}");
    }

    #[test]
    fn crlf_files_keep_their_line_endings() {
        let repo = Repo::new();
        repo.raw(
            "crlf.md",
            "---\r\ntldr: A line\r\nvalidated:\r\n  - at: 2026-01-01T00:00:00Z\r\n    by: t\r\n    how: c\r\n    ok: true\r\n---\r\nBody.\r\n",
        );
        let memory = only_memory(&repo);
        append_validation(
            &memory,
            &validation("tester", "the command", true, fixed_now()),
        )
        .expect("validating");
        let after = std::fs::read_to_string(repo.memory_path("crlf")).expect("reading back");
        assert_eq!(
            after.matches('\n').count(),
            after.matches("\r\n").count(),
            "every newline must stay a CRLF:\n{after:?}"
        );
        assert_eq!(only_memory(&repo).validated.len(), 2);
    }
}
