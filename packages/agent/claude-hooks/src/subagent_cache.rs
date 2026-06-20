//! Subagent investigation cache hooks (ENG-4665).
//!
//! `subagent-cache-lookup` (`PreToolUse` on `Agent`) serves a fresh prior
//! investigation instead of re-running a read-only subagent cold;
//! `subagent-cache-populate` (`SubagentStop`) captures each finished one. Both
//! fail OPEN and SILENT and POST to the daemon at `SUBAGENT_CACHE_URL`; when
//! that is unset or unreachable the hooks are a no-op (silent cold run), so they
//! are inert off the tailnet where the daemon lives. Kill switch:
//! `CLAUDE_CODE_DISABLE_SUBAGENT_CACHE`.
//!
//! The daemon owns Stage-1 FTS recall and the Stage-2 Haiku judge; this client
//! owns Stage-3 freshness (re-hashing each candidate's `file_deps` against the
//! working tree) and the persona hash, because only the client sees its tree.
//! `xxh64` (seed 0) matches mgrep and the hashes other developers' hooks wrote,
//! so the shared cache speaks one freshness language.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{DenyOutput, emit, flag_set, read_stdin};

/// Read-only investigators whose work is the expensive, reusable unit. Mutating
/// or open-ended agents are deliberately excluded. Matched case-sensitively
/// against `subagent_type`, so the built-in explorer is listed under both the
/// capitalized name Claude Code ships (`Explore`) and the lowercase house
/// orchestrator (`explore`). Override with the `SUBAGENT_CACHE_AGENTS` env var
/// (comma-separated).
const DEFAULT_CACHEABLE_AGENTS: &[&str] = &[
    "Explore",
    "explore",
    "codebase-locator",
    "codebase-analyzer",
    "codebase-pattern-finder",
];
/// Findings larger than this are not served via the deny reason (it would bloat
/// the model's context); fall through to a cold run. Override with
/// `SUBAGENT_CACHE_MAX_FINDINGS`.
const DEFAULT_MAX_FINDINGS: usize = 60_000;
/// Daemon base URL when `SUBAGENT_CACHE_URL` is unset: nothing listens, so the
/// hook fails open to a cold run.
const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:8787";
/// Budget for the lookup/populate calls; generous next to fail-open.
const HTTP_TIMEOUT: Duration = Duration::from_secs(3);
/// The outcome ping is pure telemetry, so it gets a tight budget.
const OUTCOME_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Serialize, Deserialize)]
struct FileDep {
    path: String,
    hash: String,
}

#[derive(Deserialize)]
struct Candidate {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    findings: String,
    #[serde(default)]
    file_deps: Vec<FileDep>,
}

/// The Stage-3 resolution for one lookup: which outcome to report and (on a
/// hit) the findings to serve. A named struct because the house clippy fork
/// forbids anonymous multi-value tuple returns.
struct Selection {
    outcome: &'static str,
    candidate_id: Option<String>,
    findings: Option<String>,
}

/// `PreToolUse(Agent)` lookup: serve a fresh cached investigation by `deny`-ing
/// the launch with the findings packed into the reason (the only way a
/// `PreToolUse` hook hands data back to the model), or stay silent for a cold run.
pub fn lookup() {
    if flag_set("CLAUDE_CODE_DISABLE_SUBAGENT_CACHE") {
        return;
    }
    let Some(input) = read_stdin() else { return };
    let Ok(payload) = serde_json::from_str::<Value>(&input) else {
        return;
    };
    let tool_input = payload.get("tool_input");
    let agent_type = tool_input
        .and_then(|t| t.get("subagent_type"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let prompt = tool_input
        .and_then(|t| t.get("prompt"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if agent_type.is_empty() || prompt.is_empty() || !cacheable_agents().contains(agent_type) {
        return;
    }

    let root = project_dir();
    let req = json!({
        "agent_type": agent_type,
        "prompt": prompt,
        "agent_def_hash": agent_def_hash(agent_type, &root),
    });
    let Some(resp) = post("/lookup", &req, HTTP_TIMEOUT) else {
        return; // daemon down or error => fail open to a cold run
    };
    let candidates: Vec<Candidate> = resp
        .get("candidates")
        .and_then(|c| serde_json::from_value(c.clone()).ok())
        .unwrap_or_default();

    let selection = select_candidate(&candidates, &root, max_findings());
    report_outcome(agent_type, &selection);
    if let Some(findings) = selection.findings {
        serve(agent_type, &findings);
    }
}

/// `SubagentStop` populate: capture a finished investigation for reuse. Reads
/// the question and the exact set of files the subagent `Read` from the
/// transcript, hashes each, and upserts one cache entry. Best-effort.
pub fn populate() {
    if flag_set("CLAUDE_CODE_DISABLE_SUBAGENT_CACHE") {
        return;
    }
    let Some(input) = read_stdin() else { return };
    let Ok(payload) = serde_json::from_str::<Value>(&input) else {
        return;
    };
    let agent_type = payload
        .get("agent_type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let findings = payload
        .get("last_assistant_message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let transcript = payload
        .get("agent_transcript_path")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if agent_type.is_empty()
        || findings.is_empty()
        || transcript.is_empty()
        || !cacheable_agents().contains(agent_type)
    {
        return;
    }

    let root = project_dir();
    let Some(prompt) = first_user_text(Path::new(transcript)) else {
        return;
    };
    // Drop deps that vanished between read and populate (hash None): an entry
    // must record only files whose content we actually captured.
    let deps: Vec<FileDep> = read_paths(Path::new(transcript), &root)
        .into_iter()
        .filter_map(|rel| hash_file(&root.join(&rel)).map(|hash| FileDep { path: rel, hash }))
        .collect();

    let req = json!({
        "agent_type": agent_type,
        "prompt": prompt,
        "findings": findings,
        "agent_def_hash": agent_def_hash(agent_type, &root),
        "model": payload.get("model").and_then(Value::as_str),
        "file_deps": deps,
    });
    let _ = post("/populate", &req, HTTP_TIMEOUT);
}

/// Pick the first fresh candidate under the findings cap, else report why none
/// served. Correctness asymmetry: a false miss is cheap, a false hit is not, so
/// anything uncertain falls through to a cold run.
fn select_candidate(candidates: &[Candidate], root: &Path, max_findings: usize) -> Selection {
    let mut stale_id: Option<String> = None;
    let mut oversize_id: Option<String> = None;
    for candidate in candidates {
        if !is_fresh(candidate, root) {
            if stale_id.is_none() {
                stale_id.clone_from(&candidate.id);
            }
            continue;
        }
        if candidate.findings.len() > max_findings {
            oversize_id.clone_from(&candidate.id);
            continue;
        }
        return Selection {
            outcome: "served",
            candidate_id: candidate.id.clone(),
            findings: Some(candidate.findings.clone()),
        };
    }
    if oversize_id.is_some() {
        return Selection {
            outcome: "oversize",
            candidate_id: oversize_id,
            findings: None,
        };
    }
    if stale_id.is_some() {
        return Selection {
            outcome: "stale",
            candidate_id: stale_id,
            findings: None,
        };
    }
    Selection {
        outcome: "miss",
        candidate_id: None,
        findings: None,
    }
}

/// A candidate is fresh when every recorded dependency still hashes to the
/// stored value. A vanished file hashes to `None` and so counts as changed.
fn is_fresh(candidate: &Candidate, root: &Path) -> bool {
    candidate
        .file_deps
        .iter()
        .all(|dep| hash_file(&root.join(&dep.path)).as_deref() == Some(dep.hash.as_str()))
}

fn serve(agent_type: &str, findings: &str) {
    let reason = format!(
        "SUBAGENT CACHE HIT (ENG-4665): a prior {agent_type} investigation answered an \
         equivalent question and every file it read is unchanged, so it was served from cache \
         instead of re-running. Use these findings directly as the subagent's result:\n\n\
         {findings}"
    );
    emit(DenyOutput {
        hook_event_name: "PreToolUse",
        permission_decision: "deny",
        permission_decision_reason: reason,
    });
}

fn report_outcome(agent_type: &str, selection: &Selection) {
    let mut body = json!({ "agent_type": agent_type, "outcome": selection.outcome });
    if let Some(id) = &selection.candidate_id {
        body["candidate_id"] = Value::String(id.clone());
    }
    let _ = post("/outcome", &body, OUTCOME_TIMEOUT);
}

/// POST JSON to the daemon, returning the decoded body or `None` on any error.
/// Fail-open is load-bearing: a cache outage must never block a subagent launch
/// or fail a finished one, so every failure mode collapses to `None`.
fn post(path: &str, body: &Value, timeout: Duration) -> Option<Value> {
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .ok()?;
    let resp = client
        .post(format!("{}{path}", daemon_url()))
        .json(body)
        .send()
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    // A 204 has an empty body; the lookup caller only reads `candidates`.
    Some(resp.json::<Value>().unwrap_or(Value::Null))
}

fn daemon_url() -> String {
    std::env::var("SUBAGENT_CACHE_URL")
        .unwrap_or_else(|_| DEFAULT_DAEMON_URL.to_owned())
        .trim_end_matches('/')
        .to_owned()
}

fn max_findings() -> usize {
    std::env::var("SUBAGENT_CACHE_MAX_FINDINGS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_FINDINGS)
}

fn cacheable_agents() -> HashSet<String> {
    match std::env::var("SUBAGENT_CACHE_AGENTS") {
        Ok(raw) if !raw.trim().is_empty() => raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => DEFAULT_CACHEABLE_AGENTS
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
    }
}

/// The repo root the hooks resolve relative paths against.
fn project_dir() -> PathBuf {
    std::env::var_os("CLAUDE_PROJECT_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Hash of the persona file, or the sentinel `none` for a built-in agent (no
/// def file). Both hooks compute this identically so the key is stable.
fn agent_def_hash(agent_type: &str, root: &Path) -> String {
    hash_file(&root.join(".claude/agents").join(format!("{agent_type}.md")))
        .unwrap_or_else(|| "none".to_owned())
}

fn hash_bytes(data: &[u8]) -> String {
    format!("xxh64:{:016x}", xxhash_rust::xxh64::xxh64(data, 0))
}

/// Content hash of a file, or `None` if it is missing/unreadable (treated as
/// changed, mirroring mgrep's vanished-file handling).
fn hash_file(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|bytes| hash_bytes(&bytes))
}

/// Repo-relative form of an absolute read path, or `None` if it is outside the
/// repo. Cross-developer sharing requires repo-relative paths: an absolute
/// `/tmp` or `/home` path never matches another machine.
fn to_repo_relative(path: &Path, root: &Path) -> Option<String> {
    let abs = path.canonicalize().ok()?;
    let root = root.canonicalize().ok()?;
    abs.strip_prefix(&root)
        .ok()
        .map(|rel| rel.to_string_lossy().into_owned())
}

/// Repo-relative paths the subagent actually `Read`, deduped and order-stable.
/// Only `Read` calls count as dependencies; grep/glob/search touch far more
/// files than they reason about.
fn read_paths(transcript: &Path, root: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(transcript) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let message = entry.get("message").unwrap_or(&entry);
        let Some(blocks) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_use")
                || block.get("name").and_then(Value::as_str) != Some("Read")
            {
                continue;
            }
            let Some(fp) = block
                .get("input")
                .and_then(|i| i.get("file_path"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            if let Some(rel) = to_repo_relative(Path::new(fp), root)
                && seen.insert(rel.clone())
            {
                out.push(rel);
            }
        }
    }
    out
}

/// The subagent's initial prompt: the first user message's text. The
/// `SubagentStop` payload does not carry it, but the transcript's first user
/// message does.
fn first_user_text(transcript: &Path) -> Option<String> {
    let content = std::fs::read_to_string(transcript).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let message = entry.get("message").unwrap_or(&entry);
        if message.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        match message.get("content") {
            Some(Value::String(text)) => return Some(text.clone()),
            Some(Value::Array(blocks)) => {
                let text: String = blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .collect();
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_owned());
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{Candidate, Selection, hash_bytes, is_fresh, select_candidate};
    use std::path::Path;

    fn candidate(id: &str, findings: &str, deps: Vec<(&str, &str)>) -> Candidate {
        Candidate {
            id: Some(id.to_owned()),
            findings: findings.to_owned(),
            file_deps: deps
                .into_iter()
                .map(|(path, hash)| super::FileDep {
                    path: path.to_owned(),
                    hash: hash.to_owned(),
                })
                .collect(),
        }
    }

    #[test]
    fn golden_xxh64_vectors() {
        // Verified against `xxhsum -H64` and mgrep's Rust xxh64.
        assert_eq!(hash_bytes(b""), "xxh64:ef46db3751d8e999");
        assert_eq!(hash_bytes(b"abc"), "xxh64:44bc2cf5ad770999");
        assert_eq!(
            hash_bytes(b"The quick brown fox jumps over the lazy dog"),
            "xxh64:0b242d361fda71bc"
        );
    }

    #[test]
    fn fresh_only_when_all_deps_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("f.rs"), b"contents").expect("write");
        let fresh = candidate("c1", "use this", vec![("f.rs", &hash_bytes(b"contents"))]);
        let stale = candidate("c2", "old", vec![("f.rs", &hash_bytes(b"old"))]);
        let gone = candidate("c3", "x", vec![("gone.rs", "xxh64:0000000000000000")]);
        assert!(is_fresh(&fresh, root));
        assert!(!is_fresh(&stale, root));
        assert!(!is_fresh(&gone, root));
    }

    #[test]
    fn serves_fresh_candidate_under_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("f.rs"), b"contents").expect("write");
        let cand = candidate("c1", "use this", vec![("f.rs", &hash_bytes(b"contents"))]);
        let Selection {
            outcome,
            candidate_id,
            findings,
        } = select_candidate(std::slice::from_ref(&cand), root, 60_000);
        assert_eq!(outcome, "served");
        assert_eq!(candidate_id.as_deref(), Some("c1"));
        assert_eq!(findings.as_deref(), Some("use this"));
    }

    #[test]
    fn reports_oversize_then_stale_then_miss() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("f.rs"), b"contents").expect("write");
        let big = candidate("big", "too large", vec![("f.rs", &hash_bytes(b"contents"))]);
        assert_eq!(
            select_candidate(std::slice::from_ref(&big), root, 3).outcome,
            "oversize"
        );
        let stale = candidate("stale", "old", vec![("f.rs", &hash_bytes(b"old"))]);
        assert_eq!(
            select_candidate(std::slice::from_ref(&stale), root, 60_000).outcome,
            "stale"
        );
        assert_eq!(select_candidate(&[], Path::new("/"), 60_000).outcome, "miss");
    }
}
