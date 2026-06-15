//! Parse Codex CLI session rollouts (one JSON object per line) into
//! [`RolloutItem`]s.
//!
//! A rollout is the full record of one session — assistant turns, tool calls,
//! tool outputs — written as JSONL under `~/.codex/sessions/YYYY/MM/DD/
//! rollout-<started>-<session_id>.jsonl`. The flat prompt log
//! (`history.jsonl`) carries user prompts only; rollouts carry everything
//! else, so this module is what makes `source=codex` cover the assistant
//! side like `claude_history` does.
//!
//! The format spans Codex CLI versions and interleaves record kinds:
//! `session_meta` and `turn_context` (ambient session/turn facts),
//! `response_item` (the conversation itself), `event_msg` (UI echoes of the
//! same content) and `compacted` (a replay of prior history). Every field is
//! therefore optional, the one place `#[serde(default)]` is the right tool:
//! this is an external, evolving schema, not internal config. A line with no
//! embeddable content is skipped; a line that is not valid JSON is skipped
//! too but logged, so one truncated line (a session still mid-write) stays
//! visible without dropping the rest of the rollout.
//!
//! A tool call and its output stay in one item: each `function_call_output`
//! / `custom_tool_call_output` is folded into the call that produced it
//! (matched by `call_id`), so a tool result is never indexed as a
//! standalone, context-free chunk. `reasoning` items carry only
//! `encrypted_content` (provider-side encryption) and are skipped: there is
//! nothing embeddable in them. `event_msg` and `compacted` lines duplicate
//! `response_item` content and are skipped too.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use snafu::ResultExt as _;
use source_meta::sanitize;

use crate::error::{ReadDirSnafu, ReadFileSnafu, Result};
use crate::record::RolloutItem;

/// Injected-context wrappers Codex prepends to user-role message blocks.
/// They are machine boilerplate (sandbox modes, AGENTS.md bodies, directory
/// listings), not anything a human said, so blocks starting with one are
/// dropped before the message is rendered.
const INJECTED_BLOCK_PREFIXES: &[&str] = &[
    "<environment_context>",
    "<goal_context>",
    "<permissions instructions>",
    "<apps_instructions>",
    "<skills_instructions>",
    "<user_instructions>",
    "<turn_aborted",
    "<system_warning",
];

/// Recursively collect `rollout-*.jsonl` files under `dir` (the
/// `~/.codex/sessions` tree is sharded as `YYYY/MM/DD/rollout-*.jsonl`).
///
/// The top-level `dir` is followed even when it is a symlink: callers name it
/// explicitly. Inside the tree, symlinks are never followed — both symlinked
/// directories and symlinked files are skipped — so a symlink planted
/// *within* the sessions tree cannot redirect the read. This mirrors the
/// claude adapter's transcript walk; like there, the caller must vet the
/// root itself when running privileged (the indexer's `safe_path_under`
/// does).
///
/// A missing directory yields nothing; a permission or I/O fault is a real
/// error (not a silently empty success). Absence is normal: most homes have
/// no Codex sessions, and the privileged fleet run walks many of them.
pub fn collect_rollouts(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).context(ReadDirSnafu {
                path: dir.to_path_buf(),
            });
        }
    };
    for entry in entries {
        let entry = entry.context(ReadDirSnafu {
            path: dir.to_path_buf(),
        })?;
        let file_type = entry.file_type().context(ReadDirSnafu {
            path: dir.to_path_buf(),
        })?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_rollouts(&path, out)?;
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
            out.push(path);
        }
    }
    Ok(())
}

/// Parse every embeddable item from one rollout file.
///
/// Lines that are not valid JSON are skipped (and logged), not fatal: a
/// rollout can carry a truncated final line from a session still writing,
/// and that must not drop the rest of the file.
///
/// # Errors
/// Returns [`Error::ReadFile`](crate::Error::ReadFile) if the file cannot be
/// read.
pub fn parse(path: &Path, host: &str, user: &str) -> Result<Vec<RolloutItem>> {
    let raw = std::fs::read_to_string(path).context(ReadFileSnafu {
        path: path.to_path_buf(),
    })?;

    let mut events = Vec::new();
    let mut skipped = 0usize;
    let mut first_bad = 0usize;
    for (index, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match parse_line(trimmed) {
            Parsed::Event(event) => events.push(event),
            // `event_msg`, `compacted`, unknown kinds: skipped by design,
            // silently — they are not damage worth reporting.
            Parsed::Ignored => {}
            Parsed::Malformed => {
                skipped += 1;
                if first_bad == 0 {
                    first_bad = index + 1;
                }
            }
        }
    }
    if skipped > 0 {
        // Visible, not silent: one bad line is tolerated, but say so.
        eprintln!(
            "[codex] {}: skipped {skipped} unparseable line(s) (first at line {first_bad})",
            path.display()
        );
    }

    // Two passes, mirroring the claude adapter: first index every tool
    // output by `call_id` (the output arrives on a later line than its
    // call), then render each item folding its output in.
    let tools = collect_tool_index(&events);

    // Ambient context, updated as the walk passes `session_meta` /
    // `turn_context` lines. A resumed session replays its source session's
    // history — including that session's own `session_meta` — into the new
    // file, so tracking the *nearest preceding* meta attributes replayed
    // items to their original session and their ids dedupe against the
    // original file's.
    let mut session_id = fallback_session_id(path);
    let mut cwd: Option<String> = None;
    let mut model: Option<String> = None;

    let mut items = Vec::new();
    for event in events {
        match event {
            Event::SessionMeta { id, cwd: meta_cwd } => {
                if let Some(id) = id {
                    session_id = id;
                }
                if meta_cwd.is_some() {
                    cwd = meta_cwd;
                }
            }
            Event::TurnContext {
                cwd: turn_cwd,
                model: turn_model,
            } => {
                if turn_cwd.is_some() {
                    cwd = turn_cwd;
                }
                if turn_model.is_some() {
                    model = turn_model;
                }
            }
            Event::Item { timestamp, item } => {
                if let Some(rendered) = render_item(item, &tools) {
                    items.push(RolloutItem {
                        host: host.to_owned(),
                        user: user.to_owned(),
                        session_id: session_id.clone(),
                        role: rendered.role,
                        record_type: rendered.record_type,
                        model: model.clone(),
                        cwd: cwd.clone(),
                        tool_name: rendered.tool_name,
                        timestamp,
                        body: rendered.body,
                    });
                }
            }
        }
    }
    Ok(items)
}

/// One rollout line reduced to what the walk needs: an ambient-context
/// update or a conversation item with its timestamp.
enum Event {
    /// A `session_meta` line: the session id (and starting directory).
    SessionMeta {
        id: Option<String>,
        cwd: Option<String>,
    },
    /// A `turn_context` line: the turn's working directory and model.
    TurnContext {
        cwd: Option<String>,
        model: Option<String>,
    },
    /// A `response_item` line: one conversation item.
    Item {
        timestamp: Option<i64>,
        item: ResponseItem,
    },
}

/// The envelope every rollout line shares: a timestamp, a record kind, and a
/// kind-shaped payload.
#[derive(Debug, Deserialize)]
struct RawLine {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    payload: Option<Value>,
}

/// A `session_meta` payload: the durable session identity.
#[derive(Debug, Deserialize)]
struct SessionMeta {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

/// A `turn_context` payload: per-turn ambient facts worth tagging.
#[derive(Debug, Deserialize)]
struct TurnContext {
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

/// One `response_item` payload. Unknown kinds (encrypted `reasoning`, future
/// additions) fall to `Other` and are skipped rather than failing the line.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponseItem {
    /// A user/assistant/developer message of text blocks.
    Message {
        #[serde(default)]
        role: Option<String>,
        #[serde(default)]
        content: Vec<TextBlock>,
    },
    /// A model-initiated tool call (`shell`, `exec_command`, MCP tools, ...).
    FunctionCall {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        arguments: Option<String>,
        #[serde(default)]
        call_id: Option<String>,
    },
    /// The output a `function_call` produced, answered by `call_id`.
    FunctionCallOutput {
        #[serde(default)]
        call_id: Option<String>,
        #[serde(default)]
        output: Option<Value>,
    },
    /// A freeform-input tool call (`apply_patch` is the common one).
    CustomToolCall {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        input: Option<String>,
        #[serde(default)]
        call_id: Option<String>,
    },
    /// The output a `custom_tool_call` produced, answered by `call_id`.
    CustomToolCallOutput {
        #[serde(default)]
        call_id: Option<String>,
        #[serde(default)]
        output: Option<Value>,
    },
    /// A provider-side web search; only the query is recorded.
    WebSearchCall {
        #[serde(default)]
        action: Option<SearchAction>,
    },
    /// Anything else (encrypted `reasoning`, `tool_search_call` boilerplate,
    /// kinds newer than this parser): nothing embeddable, skipped.
    #[serde(other)]
    Other,
}

/// One text block of a message (`input_text` / `output_text`).
#[derive(Debug, Deserialize)]
struct TextBlock {
    #[serde(default)]
    text: Option<String>,
}

/// A `web_search_call`'s action: the search query.
#[derive(Debug, Deserialize)]
struct SearchAction {
    #[serde(default)]
    query: Option<String>,
}

/// The outcome of classifying one rollout line: an event the walk consumes, a
/// kind it deliberately ignores, or actual damage worth reporting.
enum Parsed {
    /// A line the walk consumes.
    Event(Event),
    /// A kind skipped by design: `event_msg` echoes message content for the
    /// UI and `compacted` replays prior history, so both duplicate
    /// `response_item` lines; unknown kinds are formats newer than this
    /// parser.
    Ignored,
    /// Not valid JSON, or a known kind whose payload does not deserialize.
    Malformed,
}

/// Classify one line. Only a syntactically broken line (or a broken payload
/// on a kind the walk consumes) counts as malformed; everything else the
/// walk does not use is [`Parsed::Ignored`].
fn parse_line(line: &str) -> Parsed {
    let Ok(raw) = serde_json::from_str::<RawLine>(line) else {
        return Parsed::Malformed;
    };
    let Some(payload) = raw.payload else {
        return Parsed::Ignored;
    };
    match raw.kind.as_deref() {
        Some("session_meta") => serde_json::from_value::<SessionMeta>(payload).map_or(
            Parsed::Malformed,
            |meta| {
                Parsed::Event(Event::SessionMeta {
                    id: meta.id,
                    cwd: meta.cwd,
                })
            },
        ),
        Some("turn_context") => serde_json::from_value::<TurnContext>(payload).map_or(
            Parsed::Malformed,
            |context| {
                Parsed::Event(Event::TurnContext {
                    cwd: context.cwd,
                    model: context.model,
                })
            },
        ),
        Some("response_item") => serde_json::from_value::<ResponseItem>(payload).map_or(
            Parsed::Malformed,
            |item| {
                Parsed::Event(Event::Item {
                    timestamp: raw.timestamp.as_deref().and_then(parse_epoch_seconds),
                    item,
                })
            },
        ),
        _ => Parsed::Ignored,
    }
}

/// A first-pass index of one rollout's tool outputs, scanned across every
/// line so [`render_item`] can fold an output into the call that produced it
/// (the output arrives on a later line than its call).
struct ToolIndex {
    /// `call_id` to its rendered output text, for folding into the call.
    results: HashMap<String, String>,
    /// Every call id present, so an unmatched output (a truncated rollout
    /// whose call is missing) is rendered standalone rather than silently
    /// dropped.
    calls: HashSet<String>,
}

/// Build the [`ToolIndex`]: collect every call id and every output's
/// rendered text keyed by the `call_id` it answers.
fn collect_tool_index(events: &[Event]) -> ToolIndex {
    let mut results = HashMap::new();
    let mut calls = HashSet::new();
    for event in events {
        let Event::Item { item, .. } = event else {
            continue;
        };
        match item {
            ResponseItem::FunctionCall { call_id, .. }
            | ResponseItem::CustomToolCall { call_id, .. } => {
                if let Some(id) = call_id {
                    calls.insert(id.clone());
                }
            }
            ResponseItem::FunctionCallOutput { call_id, output }
            | ResponseItem::CustomToolCallOutput { call_id, output } => {
                if let Some(id) = call_id {
                    let rendered = output.clone().map_or_else(String::new, render_value);
                    // Tool output is the hostile-input path (CI logs, curl
                    // responses, cat'ed config files): sanitize and cap it
                    // before it is folded into a document body.
                    results.insert(id.clone(), sanitize::sanitize_tool_result(&rendered));
                }
            }
            _ => {}
        }
    }
    ToolIndex { results, calls }
}

/// The embeddable text of one item plus its role/type tags.
struct Rendered {
    /// The sanitized body to embed.
    body: String,
    /// Message role (`user`/`assistant`) or `tool` for an orphan output.
    role: String,
    /// The rollout item kind, the `record_type` tag.
    record_type: String,
    /// Tool name, for a tool-call item.
    tool_name: Option<String>,
}

/// Render one item to embeddable text, or `None` when it carries nothing
/// worth indexing (developer/system boilerplate, an output already folded
/// into its call, an unknown kind, an empty body).
fn render_item(item: ResponseItem, tools: &ToolIndex) -> Option<Rendered> {
    match item {
        ResponseItem::Message { role, content } => render_message(role, content),
        ResponseItem::FunctionCall {
            name,
            arguments,
            call_id,
        } => Some(render_call(
            "function_call",
            name,
            arguments,
            call_id.as_deref(),
            tools,
        )),
        ResponseItem::CustomToolCall {
            name,
            input,
            call_id,
        } => Some(render_call(
            "custom_tool_call",
            name,
            input,
            call_id.as_deref(),
            tools,
        )),
        ResponseItem::FunctionCallOutput { call_id, output } => {
            render_orphan_output("function_call_output", call_id.as_deref(), output, tools)
        }
        ResponseItem::CustomToolCallOutput { call_id, output } => {
            render_orphan_output("custom_tool_call_output", call_id.as_deref(), output, tools)
        }
        ResponseItem::WebSearchCall { action } => {
            let query = action.and_then(|action| action.query)?;
            if query.trim().is_empty() {
                return None;
            }
            Some(Rendered {
                body: sanitize::sanitize(&format!("[web_search] {query}")),
                role: "assistant".to_owned(),
                record_type: "web_search_call".to_owned(),
                tool_name: Some("web_search".to_owned()),
            })
        }
        ResponseItem::Other => None,
    }
}

/// Render a message: join its text blocks, dropping injected-context
/// boilerplate. Developer/system messages are skipped entirely — they carry
/// harness instructions, never conversation.
fn render_message(role: Option<String>, content: Vec<TextBlock>) -> Option<Rendered> {
    let role = role?;
    if role == "developer" || role == "system" {
        return None;
    }

    let mut body = String::new();
    for block in content {
        let Some(text) = block.text else {
            continue;
        };
        if is_injected_block(&text) {
            continue;
        }
        push_section(&mut body, &text);
    }
    let body = sanitize::sanitize(&body);
    if body.trim().is_empty() {
        return None;
    }
    Some(Rendered {
        body,
        role,
        record_type: "message".to_owned(),
        tool_name: None,
    })
}

/// Render a tool call with its output (looked up by `call_id`) folded in
/// right after, so the call and what it produced stay in one document. The
/// input is capped like a result: Codex routes whole `apply_patch` bodies
/// through it, and a hundred-kilobyte patch must not dominate the document.
fn render_call(
    record_type: &str,
    name: Option<String>,
    input: Option<String>,
    call_id: Option<&str>,
    tools: &ToolIndex,
) -> Rendered {
    let label = name.as_deref().unwrap_or("tool");
    let input = input.unwrap_or_default();
    let mut body = format!(
        "[tool_use {label}] {}",
        sanitize::sanitize_tool_result(&input)
    );
    let result = call_id.and_then(|id| tools.results.get(id));
    if let Some(result) = result.filter(|result| !result.is_empty()) {
        push_section(&mut body, &format!("[tool_result] {result}"));
    }
    Rendered {
        body,
        role: "assistant".to_owned(),
        record_type: record_type.to_owned(),
        tool_name: name,
    }
}

/// Render a tool output whose call is missing (a truncated rollout): surface
/// its content standalone rather than drop it. An output whose call is
/// present returns `None`; it was already folded into the call.
fn render_orphan_output(
    record_type: &str,
    call_id: Option<&str>,
    output: Option<Value>,
    tools: &ToolIndex,
) -> Option<Rendered> {
    if call_id.is_some_and(|id| tools.calls.contains(id)) {
        return None;
    }
    let rendered = sanitize::sanitize_tool_result(&output.map_or_else(String::new, render_value));
    if rendered.trim().is_empty() {
        return None;
    }
    Some(Rendered {
        body: format!("[tool_result] {rendered}"),
        role: "tool".to_owned(),
        record_type: record_type.to_owned(),
        tool_name: None,
    })
}

/// Whether a message block is injected harness context rather than something
/// a person typed.
fn is_injected_block(text: &str) -> bool {
    let trimmed = text.trim_start();
    INJECTED_BLOCK_PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
}

/// Append a section to the body, separating sections with a blank line. An
/// empty section is ignored.
fn push_section(body: &mut String, section: &str) {
    if section.is_empty() {
        return;
    }
    if !body.is_empty() {
        body.push_str("\n\n");
    }
    body.push_str(section);
}

/// Render a tool output JSON value to text: a bare string passes through,
/// anything else is compact JSON.
fn render_value(value: Value) -> String {
    match value {
        Value::String(text) => text,
        other => other.to_string(),
    }
}

/// Session id when no `session_meta` line precedes an item (a truncated
/// file): the file stem (`rollout-<started>-<session_id>`), which embeds the
/// id and stays unique per file.
fn fallback_session_id(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Parse an RFC3339 timestamp into epoch seconds, or `None` if it does not
/// parse. Mirrors the claude adapter so timestamps are consistent.
fn parse_epoch_seconds(timestamp: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|parsed| parsed.timestamp())
}
