//! Parse a Claude Code transcript (one JSON object per line) into [`Message`]s.
//!
//! The transcript format spans many Claude Code versions and interleaves record
//! kinds: `user`/`assistant` messages plus non-message markers (`ai-title`,
//! `permission-mode`, `attachment`, ...). Every field is therefore optional,
//! which is the one place `#[serde(default)]` is the right tool: this is an
//! external, evolving schema, not internal config. A line with no embeddable
//! message is skipped; a line that is not valid JSON is skipped too but logged,
//! so one truncated or corrupt line (e.g. a session still mid-write) stays
//! visible without dropping the rest of the transcript.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use snafu::ResultExt as _;
use source_meta::sanitize;

use crate::error::{ReadFileSnafu, Result};
use crate::record::{Message, MessageOrigin};

/// Parse every embeddable message from one transcript file.
///
/// Lines that are not valid JSON are skipped (and logged), not fatal: a
/// transcript can carry a truncated final line from a session still writing, or
/// an occasional corrupt line, and that must not drop the rest of the file.
///
/// # Errors
/// Returns [`Error::ReadFile`](crate::Error::ReadFile) if the file cannot be
/// read.
pub fn parse(path: &Path, origin: &MessageOrigin) -> Result<Vec<Message>> {
    let raw = std::fs::read_to_string(path).context(ReadFileSnafu {
        path: path.to_path_buf(),
    })?;

    // Parse every line first, then fold each `tool_result` into the `tool_use`
    // that produced it (the result arrives on a later line, as its own `user`
    // message). One document then carries the call and its output together, and
    // the standalone tool-result line renders empty and is dropped.
    let mut lines = Vec::new();
    let mut skipped = 0usize;
    let mut first_bad = 0usize;
    for (index, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(parsed) = serde_json::from_str::<RawLine>(trimmed) {
            lines.push(parsed);
        } else {
            skipped += 1;
            if first_bad == 0 {
                first_bad = index + 1;
            }
        }
    }
    if skipped > 0 {
        // Visible, not silent: one bad line is tolerated, but say so.
        eprintln!(
            "[claude] {}: skipped {skipped} unparseable line(s) (first at line {first_bad})",
            path.display()
        );
    }

    let tools = collect_tool_index(&lines);
    let mut messages = Vec::new();
    for line in lines {
        if let Some(message) = line.into_message(origin, &tools) {
            messages.push(message);
        }
    }
    Ok(messages)
}

/// A first-pass index of one transcript's tool blocks, scanned across every line
/// so [`render_blocks`] can fold a result into the call that produced it (the
/// result arrives on a later line than its `tool_use`).
struct ToolIndex {
    /// `tool_use` id to its rendered `tool_result` text, for folding into the call.
    results: HashMap<String, String>,
    /// Every `tool_use` id present, so an unmatched `tool_result` (a truncated or
    /// corrupt transcript whose call is missing) is rendered standalone rather
    /// than silently dropped.
    calls: HashSet<String>,
}

/// Build the [`ToolIndex`]: collect every `tool_use` id and every
/// `tool_result`'s rendered text keyed by the `tool_use_id` it answers.
fn collect_tool_index(lines: &[RawLine]) -> ToolIndex {
    let mut results = HashMap::new();
    let mut calls = HashSet::new();
    for line in lines {
        let Some(message) = &line.message else {
            continue;
        };
        let Some(Content::Blocks(blocks)) = &message.content else {
            continue;
        };
        for block in blocks {
            match block.kind.as_deref() {
                Some("tool_use") => {
                    if let Some(id) = &block.id {
                        calls.insert(id.clone());
                    }
                }
                Some("tool_result") => {
                    if let Some(id) = &block.tool_use_id {
                        let rendered = block.content.clone().map_or_else(String::new, render_value);
                        // Tool output is the hostile-input path (CI logs, curl
                        // responses, cat'ed config files): sanitize and cap it
                        // before it is folded into a document body.
                        results.insert(id.clone(), sanitize::sanitize_tool_result(&rendered));
                    }
                }
                _ => {}
            }
        }
    }
    ToolIndex { results, calls }
}

/// One transcript line. See the module docs for why every field is optional.
#[derive(Debug, Deserialize)]
struct RawLine {
    #[serde(rename = "type", default)]
    record_type: Option<String>,
    #[serde(default)]
    uuid: Option<String>,
    #[serde(rename = "parentUuid", default)]
    parent_uuid: Option<String>,
    #[serde(rename = "sessionId", default)]
    session_id: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(rename = "gitBranch", default)]
    git_branch: Option<String>,
    #[serde(default)]
    message: Option<RawMessage>,
}

impl RawLine {
    /// Project one line into a [`Message`], or `None` when the line carries no
    /// embeddable message (a title/marker line, or an empty body).
    fn into_message(self, origin: &MessageOrigin, tools: &ToolIndex) -> Option<Message> {
        let message = self.message?;
        let uuid = self.uuid?;
        let record_type = self.record_type;
        let role = message.role.clone().or_else(|| record_type.clone())?;

        let Rendered { body, tool_name } = render_content(message.content, tools);
        // Sanitize the whole rendered body (prose, thinking, and tool inputs,
        // not just tool results): strip ANSI escapes, redact credential
        // shapes, collapse blob tokens. This runs BEFORE the body is hashed in
        // `Message::into_document`, so `content_hash` is the hash of the clean
        // text and a re-sync sees previously ingested raw bodies as changed.
        let body = sanitize::sanitize(&body);
        if body.trim().is_empty() {
            return None;
        }

        let usage = message.usage;
        Some(Message {
            host: origin.host.clone(),
            user: origin.user.clone(),
            project: self.cwd.clone().unwrap_or_else(|| origin.project.clone()),
            session_id: self.session_id.unwrap_or_else(|| origin.session_id.clone()),
            uuid,
            parent_uuid: self.parent_uuid,
            record_type: record_type.unwrap_or_else(|| role.clone()),
            role,
            model: message.model,
            cwd: self.cwd,
            git_branch: self.git_branch,
            tool_name,
            input_tokens: usage.as_ref().and_then(|usage| usage.input_tokens),
            output_tokens: usage.and_then(|usage| usage.output_tokens),
            timestamp: self.timestamp.as_deref().and_then(parse_epoch_seconds),
            body,
        })
    }
}

/// The `message` object on a transcript line.
#[derive(Debug, Deserialize)]
struct RawMessage {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<Usage>,
    #[serde(default)]
    content: Option<Content>,
}

/// Token accounting on an assistant message.
#[derive(Debug, Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: Option<i64>,
    #[serde(default)]
    output_tokens: Option<i64>,
}

/// Message content: either a plain string or a list of typed blocks.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Content {
    Text(String),
    Blocks(Vec<Block>),
}

/// One content block (text, thinking, tool use, or tool result).
#[derive(Debug, Deserialize)]
struct Block {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    content: Option<Value>,
    /// A `tool_use` block's id, referenced by the matching `tool_result`.
    #[serde(default)]
    id: Option<String>,
    /// A `tool_result` block's back-reference to the `tool_use` it answers.
    #[serde(rename = "tool_use_id", default)]
    tool_use_id: Option<String>,
}

/// The embeddable text of a message plus the first tool name seen (for the
/// `tool_name` tag).
struct Rendered {
    /// The rendered body to embed.
    body: String,
    /// First tool name invoked in the message, if any.
    tool_name: Option<String>,
}

/// Render a message's content to embeddable text. Everything is included: prose,
/// thinking, and tool calls with their results folded in.
fn render_content(content: Option<Content>, tools: &ToolIndex) -> Rendered {
    match content {
        None => Rendered {
            body: String::new(),
            tool_name: None,
        },
        Some(Content::Text(text)) => Rendered {
            body: text,
            tool_name: None,
        },
        Some(Content::Blocks(blocks)) => render_blocks(blocks, tools),
    }
}

/// Render typed content blocks, joining their sections with blank lines. Each
/// `tool_use` is rendered with its matching `tool_result` (looked up by id)
/// folded in right after the call, so the call and its output stay in one
/// document. A standalone `tool_result` block is not rendered on its own; it was
/// already folded into its `tool_use`, so a line of only tool results renders
/// empty and is dropped by the caller.
fn render_blocks(blocks: Vec<Block>, tools: &ToolIndex) -> Rendered {
    let mut body = String::new();
    let mut tool_name = None;

    for block in blocks {
        let kind = block.kind.as_deref().unwrap_or_default();
        match kind {
            "text" => push_section(&mut body, block.text.as_deref()),
            "thinking" => push_section(&mut body, block.thinking.as_deref()),
            "tool_use" => {
                // Keep the first tool name seen; `or_else` only clones when unset.
                tool_name = tool_name.or_else(|| block.name.clone());
                let name = block.name.as_deref().unwrap_or("tool");
                let result = block.id.as_deref().and_then(|id| tools.results.get(id));
                let input = block.input.map_or_else(String::new, render_value);
                push_section(&mut body, Some(&format!("[tool_use {name}] {input}")));
                // Fold the call's output into the same document.
                if let Some(result) = result.filter(|result| !result.is_empty()) {
                    push_section(&mut body, Some(&format!("[tool_result] {result}")));
                }
            }
            "tool_result" => {
                // Normally folded into its `tool_use` above, so skip it here. But
                // if the matching call is absent (a truncated or corrupt
                // transcript), render it standalone rather than drop its content.
                let folded = block
                    .tool_use_id
                    .as_deref()
                    .is_some_and(|id| tools.calls.contains(id));
                if !folded {
                    let rendered = block.content.map_or_else(String::new, render_value);
                    let rendered = sanitize::sanitize_tool_result(&rendered);
                    push_section(&mut body, Some(&format!("[tool_result] {rendered}")));
                }
            }
            other => {
                if let Some(text) = block.text.as_deref() {
                    push_section(&mut body, Some(&format!("[{other}] {text}")));
                }
            }
        }
    }
    Rendered { body, tool_name }
}

/// Append a section to the body, separating sections with a blank line. A
/// missing or empty section is ignored.
fn push_section(body: &mut String, section: Option<&str>) {
    let Some(section) = section else {
        return;
    };
    if section.is_empty() {
        return;
    }
    if !body.is_empty() {
        body.push_str("\n\n");
    }
    body.push_str(section);
}

/// Render a tool input/result JSON value to text: a bare string passes through,
/// an array of `{text}` blocks joins their text, anything else is compact JSON.
fn render_value(value: Value) -> String {
    match value {
        Value::String(text) => text,
        Value::Array(items) => items
            .into_iter()
            .map(render_array_item)
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}

/// Render one element of a tool-result content array.
fn render_array_item(item: Value) -> String {
    match item {
        Value::String(text) => text,
        Value::Object(mut map) => map
            .remove("text")
            .and_then(|text| text.as_str().map(str::to_owned))
            .unwrap_or_else(|| Value::Object(map).to_string()),
        other => other.to_string(),
    }
}

/// Parse an RFC3339 timestamp into epoch seconds, or `None` if it does not
/// parse. Mirrors the linear-export adapter so timestamps are consistent.
fn parse_epoch_seconds(timestamp: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|parsed| parsed.timestamp())
}

#[cfg(test)]
mod tests {
    #![expect(clippy::expect_used, reason = "tests assert observable parse outcomes")]

    use std::io::Write as _;

    use super::{MessageOrigin, parse};

    fn origin() -> MessageOrigin {
        MessageOrigin {
            host: "h".to_owned(),
            user: "u".to_owned(),
            project: "p".to_owned(),
            session_id: "s".to_owned(),
        }
    }

    fn transcript(lines: &[&str]) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().expect("tempfile");
        for line in lines {
            file.write_all(line.as_bytes()).expect("write");
            file.write_all(b"\n").expect("write");
        }
        file.flush().expect("flush");
        file
    }

    #[test]
    fn tool_result_is_folded_into_its_tool_use() {
        let file = transcript(&[
            r#"{"type":"assistant","uuid":"a1","sessionId":"s1","message":{"role":"assistant","content":[{"type":"text","text":"running it"},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]}}"#,
            r#"{"type":"user","uuid":"u1","sessionId":"s1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"OUTPUT-MARKER"}]}}"#,
        ]);

        let messages = parse(file.path(), &origin()).expect("parse");
        assert_eq!(
            messages.len(),
            1,
            "the standalone tool_result line is not its own document"
        );
        let body = &messages[0].body;
        assert_eq!(messages[0].uuid, "a1");
        assert!(body.contains("[tool_use Bash]"), "call present: {body}");
        assert!(body.contains("ls"), "call input present: {body}");
        assert!(
            body.contains("[tool_result] OUTPUT-MARKER"),
            "result folded in: {body}"
        );
    }

    #[test]
    fn orphan_tool_result_renders_standalone() {
        // A tool_result whose tool_use never appears (a truncated transcript)
        // must still surface its content rather than vanish.
        let file = transcript(&[
            r#"{"type":"user","uuid":"u1","sessionId":"s1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"MISSING","content":"ORPHAN-OUTPUT"}]}}"#,
        ]);
        let messages = parse(file.path(), &origin()).expect("parse");
        assert_eq!(
            messages.len(),
            1,
            "an orphan tool_result still yields a document"
        );
        assert!(
            messages[0].body.contains("[tool_result] ORPHAN-OUTPUT"),
            "{}",
            messages[0].body
        );
    }

    #[test]
    fn tool_use_without_a_result_still_renders() {
        let file = transcript(&[
            r#"{"type":"assistant","uuid":"a1","sessionId":"s1","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_x","name":"Read","input":{"path":"f"}}]}}"#,
        ]);
        let messages = parse(file.path(), &origin()).expect("parse");
        assert_eq!(messages.len(), 1);
        assert!(
            messages[0].body.contains("[tool_use Read]"),
            "{}",
            messages[0].body
        );
        assert!(!messages[0].body.contains("[tool_result]"));
    }
}
