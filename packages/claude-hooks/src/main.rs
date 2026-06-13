//! Claude Code hook commands, one compiled binary with three subcommands that
//! replace the old hand-rolled `writeShellScript` hooks in
//! `packages/claude-code`. Every hook fails OPEN and SILENT: any missing input,
//! parse error, or kill-switch returns with no stdout, because a noisy or broken
//! hook is strictly worse than no hook.
//!
//! Tool paths and the baked primary-checkout default are passed by the
//! claude-code wrapper via env (`IX_GIT`, `IX_SEARCH`,
//! `IX_DEFAULT_PRIMARY_CHECKOUTS`); user-facing knobs keep their
//! `CLAUDE_CODE_*` names.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use chrono::{DateTime, SecondsFormat};
use serde::Serialize;
use serde_json::Value;

/// `SessionStart` digest cap (~1500 tokens), inside the 10,000-char
/// `additionalContext` limit.
const DIGEST_CAP: usize = 6000;
/// Prompt-priors cap (~1200 tokens).
const PRIORS_CAP: usize = 4800;
/// Only inject corpus hits at or above this relevance score.
const SCORE_GATE: f64 = 0.70;
/// Ambient recall is measured net-negative below this many words.
const MIN_WORDS: usize = 8;

/// Fleet-specific nouns; a prompt without one embeds near everything and pulls
/// vendored-code noise, so it gets no query. Cheap allowlist, deliberately dumb.
const FLEET_NOUNS: &[&str] = &[
    "index",
    "indexer",
    "colmena",
    "mixedbread",
    "mgrep",
    "flake",
    "flakes",
    "nixos",
    "fleet",
    "deploy",
    "claude",
    "codex",
    "kernel",
    "iceberg",
    "parquet",
    "atuin",
    "graphite",
    "worktree",
    "worktrees",
    "rebase",
    "linear",
    "slack",
    "github",
    "tailscale",
    "cargo",
    "clippy",
    "nushell",
    "symphony",
    "vmkit",
    "cachix",
];

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("session-digest") => session_digest(),
        Some("worktree-guard") => worktree_guard(),
        Some("prompt-priors") => prompt_priors(),
        other => {
            eprintln!("claude-hooks: unknown subcommand {other:?}");
            return ExitCode::from(2);
        }
    }
    ExitCode::SUCCESS
}

// --- shared helpers ---

/// True when an env var is present and non-empty (the kill-switch convention).
fn flag_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/var/empty"), PathBuf::from)
}

fn read_stdin() -> Option<String> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf).ok()?;
    Some(buf)
}

fn cap_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextOutput {
    hook_event_name: &'static str,
    additional_context: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DenyOutput {
    hook_event_name: &'static str,
    permission_decision: &'static str,
    permission_decision_reason: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Wrap<T> {
    hook_specific_output: T,
}

fn emit<T: Serialize>(inner: T) {
    if let Ok(s) = serde_json::to_string(&Wrap {
        hook_specific_output: inner,
    }) {
        println!("{s}");
    }
}

// --- session-digest ---

fn session_digest() {
    if flag_set("CLAUDE_CODE_DISABLE_CONTEXT_DIGEST") {
        return;
    }
    let path = home().join(".cache/ix/context-digest.md");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let context = cap_chars(&text, DIGEST_CAP);
    if context.is_empty() {
        return;
    }
    emit(ContextOutput {
        hook_event_name: "SessionStart",
        additional_context: context,
    });
}

// --- worktree-guard ---

fn worktree_guard() {
    if flag_set("CLAUDE_CODE_DISABLE_WORKTREE_GUARD") {
        return;
    }
    let Some(input) = read_stdin() else { return };
    let Ok(payload) = serde_json::from_str::<Value>(&input) else {
        return;
    };
    let Some(file) = payload
        .get("tool_input")
        .and_then(|t| t.get("file_path").or_else(|| t.get("notebook_path")))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return;
    };

    // Judge the target, never the session: a relative path resolves against the
    // payload cwd, an absolute one stands alone.
    let target = if file.starts_with('/') {
        PathBuf::from(file)
    } else {
        let cwd = payload
            .get("cwd")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("PWD").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        cwd.join(file)
    };

    // A new file's parent may not exist yet; the nearest existing ancestor's
    // repo decides.
    let mut dir = target
        .parent()
        .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
    while dir.as_path() != Path::new("/") && !dir.is_dir() {
        dir = dir
            .parent()
            .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
    }

    let git = std::env::var("IX_GIT").unwrap_or_else(|_| "git".to_owned());
    let Some(gitdir) = git_rev_parse(&git, &dir, "--git-dir") else {
        return;
    };
    let Some(common) = git_rev_parse(&git, &dir, "--git-common-dir") else {
        return;
    };
    // Linked worktree: private git-dir differs from the shared common dir.
    if gitdir != common {
        return;
    }
    let Some(toplevel) = git_rev_parse(&git, &dir, "--show-toplevel") else {
        return;
    };

    if matches_protected(&toplevel, &primary_checkouts()) {
        emit(DenyOutput {
            hook_event_name: "PreToolUse",
            permission_decision: "deny",
            permission_decision_reason: format!(
                "Refusing to edit {toplevel}: it is a primary checkout, not a worktree, \
                 and other work may be in flight there. Create a dedicated worktree \
                 (`git -C {toplevel} worktree add <dir> -b <branch> origin/main`) and edit \
                 the file there instead. Reads are always fine."
            ),
        });
    }
}

fn git_rev_parse(git: &str, dir: &Path, what: &str) -> Option<String> {
    let out = Command::new(git)
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--path-format=absolute", what])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// User override first, else the wrapper-baked default; colon-separated, empties
/// dropped. Empty list means no guard.
fn primary_checkouts() -> Vec<String> {
    let raw = std::env::var("CLAUDE_CODE_PRIMARY_CHECKOUTS")
        .or_else(|_| std::env::var("IX_DEFAULT_PRIMARY_CHECKOUTS"))
        .unwrap_or_default();
    raw.split(':')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Shell `case`-glob semantics: `*` crosses `/` (glob's default
/// `require_literal_separator = false`), matching the whole `toplevel`.
fn matches_protected(toplevel: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .any(|p| glob::Pattern::new(p).is_ok_and(|pat| pat.matches(toplevel)))
}

// --- prompt-priors ---

fn prompt_priors() {
    if flag_set("CLAUDE_CODE_DISABLE_PROMPT_PRIORS") {
        return;
    }
    let Some(input) = read_stdin() else { return };
    let prompt = serde_json::from_str::<Value>(&input)
        .ok()
        .and_then(|v| v.get("prompt").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default();
    if prompt.is_empty()
        || !passes_word_gate(&prompt)
        || !has_fleet_noun(&prompt)
        || !has_credential()
    {
        return;
    }
    let Some(hits) = run_search(&prompt) else {
        return;
    };
    let Some(context) = render_priors(&hits) else {
        return;
    };
    emit(ContextOutput {
        hook_event_name: "UserPromptSubmit",
        additional_context: context,
    });
}

fn passes_word_gate(prompt: &str) -> bool {
    prompt.split_whitespace().count() >= MIN_WORDS
}

fn has_fleet_noun(prompt: &str) -> bool {
    prompt
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .any(|t| FLEET_NOUNS.contains(&t.to_ascii_lowercase().as_str()))
}

fn has_credential() -> bool {
    flag_set("MXBAI_API_KEY") || home().join(".mgrep/token.json").exists()
}

fn run_search(prompt: &str) -> Option<Vec<Value>> {
    let search = std::env::var("IX_SEARCH").ok()?;
    let mut child = Command::new(search)
        .arg(prompt)
        .args([
            "--json",
            "--compact",
            "--no-rerank",
            "--max-count",
            "3",
            "--source",
            "claude_history,shell,github",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    // Single-shot under a hard 2s budget; a slow store, dead network, or zero
    // hits must never stall the prompt. Output is tiny (--max-count 3 --compact),
    // so the unread pipe cannot fill before the child exits.
    let started = Instant::now();
    let status = loop {
        match child.try_wait().ok()? {
            Some(status) => break status,
            None if started.elapsed() >= Duration::from_secs(2) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    if !status.success() {
        return None;
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    serde_json::from_str::<Value>(&out)
        .ok()?
        .as_array()
        .cloned()
}

fn render_priors(hits: &[Value]) -> Option<String> {
    let snippets: Vec<String> = hits
        .iter()
        .filter(|h| h.get("score").and_then(Value::as_f64).unwrap_or(0.0) >= SCORE_GATE)
        .map(|h| {
            let path = h.get("path").and_then(Value::as_str).unwrap_or("");
            let text = h.get("text").and_then(Value::as_str).unwrap_or("");
            format!("[{}] {path}\n{text}", provenance(h))
        })
        .collect();
    if snippets.is_empty() {
        return None;
    }
    let body = snippets.join("\n\n");
    Some(cap_chars(
        &format!(
            "Possibly relevant fleet history (ambient, score-gated; may be stale or from \
             another user, verify before relying on it):\n\n{body}"
        ),
        PRIORS_CAP,
    ))
}

/// `source [by user] [timestamp] score N`, matching the old jq projection so the
/// model can discount stale or cross-user content.
fn provenance(hit: &Value) -> String {
    let mut parts = vec![
        hit.get("source")
            .and_then(Value::as_str)
            .unwrap_or("corpus")
            .to_owned(),
    ];
    if let Some(user) = hit.get("user").and_then(Value::as_str) {
        parts.push(format!("by {user}"));
    }
    if let Some(dt) = hit
        .get("timestamp")
        .and_then(Value::as_i64)
        .and_then(|ts| DateTime::from_timestamp(ts, 0))
    {
        parts.push(dt.to_rfc3339_opts(SecondsFormat::Secs, true));
    }
    let score = hit.get("score").and_then(Value::as_f64).unwrap_or(0.0);
    parts.push(format!("score {}", (score * 100.0).floor() / 100.0));
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::{
        cap_chars, has_fleet_noun, matches_protected, passes_word_gate, provenance, render_priors,
    };
    use serde_json::json;

    #[test]
    fn cap_chars_truncates_on_char_count() {
        assert_eq!(cap_chars(&"x".repeat(9000), 6000).chars().count(), 6000);
        assert_eq!(cap_chars("short", 6000), "short");
    }

    #[test]
    fn word_gate() {
        assert!(!passes_word_gate("fix this typo now"));
        assert!(passes_word_gate(
            "how do we deploy the fleet to every host today"
        ));
    }

    #[test]
    fn fleet_noun_gate() {
        assert!(has_fleet_noun(
            "how do we deploy the fleet with colmena to every host"
        ));
        assert!(!has_fleet_noun(
            "please rename this function to something clearer for readability"
        ));
        // whole-word, case-insensitive
        assert!(has_fleet_noun("rebuild the NixOS image"));
        assert!(!has_fleet_noun("reindexing is unrelated")); // substring, not a word
    }

    #[test]
    fn protected_glob_crosses_slash() {
        let pats = vec!["/home/*/index".to_owned(), "/home/*/ix".to_owned()];
        assert!(matches_protected("/home/andrew/index", &pats));
        // `*` crosses `/` like shell `case`
        assert!(matches_protected("/home/a/b/index", &pats));
        assert!(!matches_protected("/tmp/scratch/index-clone", &pats));
        assert!(matches_protected(
            "/srv/x",
            &["/srv/x".to_owned()] // exact path, no glob
        ));
    }

    #[test]
    fn priors_score_gate_and_header() {
        let hits = vec![
            json!({ "score": 0.82, "source": "github", "path": "a", "text": "keep me" }),
            json!({ "score": 0.55, "source": "shell", "path": "b", "text": "drop me" }),
        ];
        let out = render_priors(&hits).expect("a hit above gate");
        assert!(out.starts_with("Possibly relevant fleet history"));
        assert!(out.contains("keep me"));
        assert!(!out.contains("drop me"));
    }

    #[test]
    fn priors_none_when_all_below_gate() {
        let hits = vec![json!({ "score": 0.69, "path": "a", "text": "t" })];
        assert!(render_priors(&hits).is_none());
    }

    #[test]
    fn priors_capped() {
        let hits = vec![json!({
            "score": 0.9,
            "source": "github",
            "path": "p",
            "text": "x".repeat(9000),
        })];
        let out = render_priors(&hits).expect("hit");
        assert_eq!(out.chars().count(), 4800);
    }

    #[test]
    fn provenance_formats_fields() {
        let p = provenance(&json!({
            "source": "claude_history",
            "user": "andrew",
            "score": 0.726,
        }));
        assert_eq!(p, "claude_history by andrew score 0.72");
    }
}
