//! Typed result of a single tool call, carried on `Message.result`
//! end to end (DB row -> wire -> client).
//!
//! This replaces the old `tool_output: Option<String>`, which collapsed
//! four distinct states into one untyped string: a running call, a call
//! that finished with no payload, a real value, and an outright failure.
//! Codex hands us rich typed status for every tool kind
//! (`mcpToolCall { status, result, error }`,
//! `commandExecution { status, exitCode, ... }`), but the bridge used to
//! read only `result` and `serde_json::to_string` it. When a codex MCP
//! call failed, codex set `result: null` and put the reason in `error`;
//! stringifying the null produced the literal `"null"` the UI rendered,
//! and the `error` was dropped on the floor. This type keeps the status
//! and the failure reason so the client can render them.
//!
//! `ToolResult::from_*` are the single source of truth for mapping a
//! codex item payload onto this shape; the live `codex_bridge` calls
//! them. (The separate `engine_codex` adapter emits a different,
//! camelCase cross-language event contract that is not yet consumed into
//! tool-result rendering, so it is intentionally left untouched.)

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Outcome of a tool call. Serialized with a `status` discriminator so
/// it maps 1:1 onto a TypeScript discriminated union on the client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolResult {
    /// Still in flight. `display` carries any partial output streamed so
    /// far (shell stdout/stderr) so the UI can show progress.
    Running {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<String>,
    },
    /// Completed with a payload.
    Ok {
        /// Flattened, human-readable text, computed here at the owner
        /// boundary so the client never re-derives it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        /// The structured payload as the engine delivered it (MCP content
        /// blocks, web-search results), kept for richer rendering.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<i64>,
    },
    /// Completed successfully but produced no payload. Distinct from a
    /// missing result so the UI can say "(no output)" rather than going
    /// blank or, as before, printing the string `null`.
    Empty {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<i64>,
    },
    /// Failed. `message` is the engine-supplied reason.
    Error {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<i64>,
    },
    /// Interrupted or cancelled before it could complete.
    Cancelled,
}

impl ToolResult {
    /// A running call, optionally with partial output streamed so far.
    pub fn running(partial: Option<String>) -> Self {
        Self::Running {
            display: partial.filter(|s| !s.trim().is_empty()),
        }
    }

    /// Map a codex `mcpToolCall` item payload onto a result. Returns
    /// `None` while the call is still in progress so the caller renders
    /// `Running`.
    pub fn from_mcp(item: &Value) -> Option<Self> {
        let status = item.get("status").and_then(Value::as_str).unwrap_or("");
        if status == "inProgress" {
            return None;
        }
        let duration_ms = item.get("durationMs").and_then(Value::as_i64);
        // Codex carries the failure reason out-of-band in `error`,
        // separate from `result`; honor it before reading the payload.
        if let Some(message) = item
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
        {
            return Some(Self::Error {
                message: message.to_owned(),
                display: None,
                content: None,
                exit_code: None,
                duration_ms,
            });
        }
        if status == "failed" {
            return Some(Self::Error {
                message: "MCP tool call failed".to_owned(),
                display: None,
                content: None,
                exit_code: None,
                duration_ms,
            });
        }
        match item.get("result") {
            Some(result) if !result.is_null() => {
                // Prefer the text content blocks; fall back to the
                // structured payload when a tool returns data with no
                // text blocks.
                let display = result
                    .get("content")
                    .map(flatten_textual)
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        result
                            .get("structuredContent")
                            .filter(|v| !v.is_null())
                            .map(flatten_textual)
                            .filter(|s| !s.is_empty())
                    });
                Some(Self::Ok {
                    display,
                    content: Some(result.clone()),
                    exit_code: None,
                    duration_ms,
                })
            }
            _ => Some(Self::Empty { duration_ms }),
        }
    }

    /// Map a codex `commandExecution` item plus the output accumulated by
    /// the bridge onto a result. Returns `None` while in progress.
    pub fn from_command(item: &Value, output: &str) -> Option<Self> {
        let status = item.get("status").and_then(Value::as_str).unwrap_or("");
        let exit_code = item.get("exitCode").and_then(Value::as_i64);
        let duration_ms = item.get("durationMs").and_then(Value::as_i64);
        let display = nonempty(output);
        match status {
            "" | "inProgress" => None,
            "declined" => Some(Self::Cancelled),
            "failed" => Some(Self::Error {
                message: match exit_code {
                    Some(code) => format!("command failed (exit {code})"),
                    None => "command failed".to_owned(),
                },
                display,
                content: None,
                exit_code,
                duration_ms,
            }),
            // "completed" (and any future success spelling)
            _ => match exit_code {
                Some(code) if code != 0 => Some(Self::Error {
                    message: format!("command exited with code {code}"),
                    display,
                    content: None,
                    exit_code,
                    duration_ms,
                }),
                _ if display.is_none() => Some(Self::Empty { duration_ms }),
                _ => Some(Self::Ok {
                    display,
                    content: None,
                    exit_code,
                    duration_ms,
                }),
            },
        }
    }

    /// Map a codex `webSearch` item (results array) onto a result.
    pub fn from_web_search(item: &Value) -> Option<Self> {
        let status = item.get("status").and_then(Value::as_str).unwrap_or("");
        if status == "inProgress" {
            return None;
        }
        let duration_ms = item.get("durationMs").and_then(Value::as_i64);
        match item.get("results") {
            Some(results) if !results.is_null() => Some(Self::Ok {
                display: nonempty(&flatten_textual(results)),
                content: Some(results.clone()),
                exit_code: None,
                duration_ms,
            }),
            _ => Some(Self::Empty { duration_ms }),
        }
    }

    /// Map a codex `fileChange` item onto a result. The diff itself
    /// rides `Message.patch`; this records whether the apply succeeded so
    /// a failed or declined patch can't read as a clean diff. Falls back
    /// to `Ok` when the item carries no status (older codex shapes).
    pub fn from_file_change(item: &Value) -> Option<Self> {
        // The codex fileChange item carries no durationMs (unlike command
        // and mcp items), so the result never records one.
        match item.get("status").and_then(Value::as_str).unwrap_or("") {
            "inProgress" => None,
            "declined" => Some(Self::Cancelled),
            "failed" => Some(Self::Error {
                message: "patch apply failed".to_owned(),
                display: None,
                content: item.get("changes").cloned(),
                exit_code: None,
                duration_ms: None,
            }),
            _ => item
                .get("changes")
                .map(|changes| Self::ok_payload(changes.clone(), None)),
        }
    }

    /// A flat payload (file-change diff list, etc.) that has completed.
    pub fn ok_payload(content: Value, duration_ms: Option<i64>) -> Self {
        Self::Ok {
            display: None,
            content: Some(content),
            exit_code: None,
            duration_ms,
        }
    }

    /// Reconstruct a result from a legacy `tool_output` string read off a
    /// pre-migration DB row. The original status is unknowable, so a
    /// present value becomes `Ok` and an absent/`"null"` one `Empty`.
    pub fn from_legacy(tool_output: Option<String>) -> Option<Self> {
        let raw = tool_output?;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "null" {
            return Some(Self::Empty { duration_ms: None });
        }
        let display = match serde_json::from_str::<Value>(trimmed) {
            Ok(value) => nonempty(&flatten_textual(&value)),
            Err(_) => Some(raw.clone()),
        };
        Some(Self::Ok {
            display,
            content: None,
            exit_code: None,
            duration_ms: None,
        })
    }
}

fn nonempty(s: &str) -> Option<String> {
    (!s.trim().is_empty()).then(|| s.to_owned())
}

/// Pull the human-readable text out of MCP content blocks and nested
/// envelopes. Mirrors the client's historical `flattenTextual` so codex
/// outputs that rendered before keep rendering the same way, now computed
/// once on the server.
pub fn flatten_textual(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(flatten_textual)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) => {
            match map.get("content") {
                Some(Value::String(s)) => return s.clone(),
                Some(content @ Value::Array(_)) => return flatten_textual(content),
                _ => {}
            }
            if let Some(Value::String(s)) = map.get("text") {
                return s.clone();
            }
            serde_json::to_string_pretty(v).unwrap_or_default()
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mcp_completed_with_text_blocks_is_ok() {
        let item = json!({
            "type": "mcpToolCall",
            "status": "completed",
            "result": { "content": [{ "type": "text", "text": "4" }] },
            "durationMs": 12,
        });
        assert_eq!(
            ToolResult::from_mcp(&item),
            Some(ToolResult::Ok {
                display: Some("4".to_owned()),
                content: Some(json!({ "content": [{ "type": "text", "text": "4" }] })),
                exit_code: None,
                duration_ms: Some(12),
            })
        );
    }

    #[test]
    fn mcp_completed_with_null_result_is_empty_not_the_string_null() {
        // The original bug: codex sends result: null, the bridge used to
        // stringify it to "null" and render that.
        let item = json!({ "type": "mcpToolCall", "status": "completed", "result": null });
        assert_eq!(
            ToolResult::from_mcp(&item),
            Some(ToolResult::Empty { duration_ms: None })
        );
    }

    #[test]
    fn mcp_failed_surfaces_the_error_message() {
        let item = json!({
            "type": "mcpToolCall",
            "status": "failed",
            "result": null,
            "error": { "message": "tool exploded" },
        });
        assert_eq!(
            ToolResult::from_mcp(&item),
            Some(ToolResult::Error {
                message: "tool exploded".to_owned(),
                display: None,
                content: None,
                exit_code: None,
                duration_ms: None,
            })
        );
    }

    #[test]
    fn mcp_in_progress_is_running() {
        let item = json!({ "type": "mcpToolCall", "status": "inProgress" });
        assert_eq!(ToolResult::from_mcp(&item), None);
    }

    #[test]
    fn command_nonzero_exit_is_error_with_code() {
        let item = json!({ "type": "commandExecution", "status": "completed", "exitCode": 2 });
        assert_eq!(
            ToolResult::from_command(&item, "boom\n"),
            Some(ToolResult::Error {
                message: "command exited with code 2".to_owned(),
                display: Some("boom\n".to_owned()),
                content: None,
                exit_code: Some(2),
                duration_ms: None,
            })
        );
    }

    #[test]
    fn command_clean_exit_with_output_is_ok() {
        let item = json!({ "type": "commandExecution", "status": "completed", "exitCode": 0 });
        assert_eq!(
            ToolResult::from_command(&item, "hi\n"),
            Some(ToolResult::Ok {
                display: Some("hi\n".to_owned()),
                content: None,
                exit_code: Some(0),
                duration_ms: None,
            })
        );
    }

    #[test]
    fn round_trips_through_json() {
        let result = ToolResult::Error {
            message: "nope".to_owned(),
            display: Some("stderr".to_owned()),
            content: None,
            exit_code: Some(1),
            duration_ms: Some(5),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"status\":\"error\""));
        assert_eq!(serde_json::from_str::<ToolResult>(&json).unwrap(), result);
    }

    #[test]
    fn legacy_null_string_becomes_empty() {
        assert_eq!(
            ToolResult::from_legacy(Some("null".to_owned())),
            Some(ToolResult::Empty { duration_ms: None })
        );
        assert_eq!(ToolResult::from_legacy(None), None);
    }
}
