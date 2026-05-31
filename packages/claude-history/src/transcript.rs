//! Parse a Claude Code transcript (one JSON object per line) into [`Message`]s.
//!
//! The transcript format spans many Claude Code versions and interleaves record
//! kinds: `user`/`assistant` messages plus non-message markers (`ai-title`,
//! `permission-mode`, `attachment`, ...). Every field is therefore optional,
//! which is the one place `#[serde(default)]` is the right tool: this is an
//! external, evolving schema, not internal config. A line with no embeddable
//! message is skipped; a line that is not valid JSON is a typed error, because a
//! corrupt transcript should be visible, not silently truncated.

use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use snafu::ResultExt as _;

use crate::error::{ParseLineSnafu, ReadFileSnafu, Result};
use crate::record::{Message, MessageOrigin};

/// Parse every embeddable message from one transcript file.
///
/// # Errors
/// Returns [`Error::ReadFile`](crate::Error::ReadFile) if the file cannot be
/// read, or [`Error::ParseLine`](crate::Error::ParseLine) if any line is not
/// valid JSON.
pub fn parse(path: &Path, origin: &MessageOrigin) -> Result<Vec<Message>> {
    let raw = std::fs::read_to_string(path).context(ReadFileSnafu { path: path.to_path_buf() })?;

    let mut messages = Vec::new();
    for (index, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: RawLine = serde_json::from_str(trimmed)
            .with_context(|_| ParseLineSnafu { path: path.to_path_buf(), line: index + 1 })?;
        if let Some(message) = parsed.into_message(origin) {
            messages.push(message);
        }
    }
    Ok(messages)
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
    fn into_message(self, origin: &MessageOrigin) -> Option<Message> {
        let message = self.message?;
        let uuid = self.uuid?;
        let record_type = self.record_type;
        let role = message.role.clone().or_else(|| record_type.clone())?;

        let (body, tool_name) = render_content(message.content);
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
}

/// Render a message's content to embeddable text, returning the text and the
/// first tool name seen (for the `tool_name` tag). Everything is included:
/// prose, thinking, tool calls, and tool results.
fn render_content(content: Option<Content>) -> (String, Option<String>) {
    match content {
        None => (String::new(), None),
        Some(Content::Text(text)) => (text, None),
        Some(Content::Blocks(blocks)) => render_blocks(blocks),
    }
}

/// Render typed content blocks, joining their sections with blank lines.
fn render_blocks(blocks: Vec<Block>) -> (String, Option<String>) {
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
                let input = block.input.map_or_else(String::new, render_value);
                push_section(&mut body, Some(&format!("[tool_use {name}] {input}")));
            }
            "tool_result" => {
                let rendered = block.content.map_or_else(String::new, render_value);
                push_section(&mut body, Some(&format!("[tool_result] {rendered}")));
            }
            other => {
                if let Some(text) = block.text.as_deref() {
                    push_section(&mut body, Some(&format!("[{other}] {text}")));
                }
            }
        }
    }
    (body, tool_name)
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
