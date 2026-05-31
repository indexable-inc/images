//! Canonical metadata key names, the single source of truth shared by adapters
//! (which write them) and the filter builder (which queries them).
//!
//! Keeping them as `const`s, rather than string literals scattered across
//! crates, means a query can never target a key an adapter never writes without
//! the mismatch being visible in one place.

/// Which corpus a record came from. The primary scope filter key.
pub const SOURCE: &str = "source";
/// sha256 of the embedded body; drives skip-if-unchanged reconcile.
pub const CONTENT_HASH: &str = "content_hash";
/// Human display label for a record.
pub const TITLE: &str = "title";

// Code.
/// Repository slug (git remote, or directory name when there is no remote).
pub const REPO: &str = "repo";
/// Repo-relative path of a code file.
pub const PATH: &str = "path";

// Common.
/// Epoch-second timestamp, the primary recency axis.
pub const TIMESTAMP: &str = "timestamp";

// Slack.
/// Slack channel id (stable across renames).
pub const CHANNEL_ID: &str = "channel_id";
/// Slack channel name (display).
pub const CHANNEL_NAME: &str = "channel_name";
/// Slack author display names in a thread.
pub const AUTHORS: &str = "authors";
/// Whether the thread is in an external / Slack-Connect channel.
pub const IS_EXTERNAL: &str = "is_external";
/// Whether every message in the thread is from a bot/integration.
pub const IS_BOT_THREAD: &str = "is_bot_thread";

// Claude Code agent history.
/// Short hostname the transcript was recorded on.
pub const HOST: &str = "host";
/// Local OS user that owns the transcript.
pub const USER: &str = "user";
/// Project slug (the real working-directory path the session ran in).
pub const PROJECT: &str = "project";
/// Claude Code session id (one transcript file).
pub const SESSION_ID: &str = "session_id";
/// Stable per-message uuid assigned by Claude Code.
pub const MESSAGE_UUID: &str = "message_uuid";
/// Parent message uuid, threading the conversation.
pub const PARENT_UUID: &str = "parent_uuid";
/// Message role (`user`/`assistant`/`system`).
pub const ROLE: &str = "role";
/// Transcript record type (`user`/`assistant`/`system`/...).
pub const RECORD_TYPE: &str = "record_type";
/// Model id for an assistant message.
pub const MODEL: &str = "model";
/// Working directory the session ran in.
pub const CWD: &str = "cwd";
/// Git branch checked out during the message, when recorded.
pub const GIT_BRANCH: &str = "git_branch";
/// Tool name for a tool-use / tool-result message, when present.
pub const TOOL_NAME: &str = "tool_name";
/// Assistant input token count, when recorded.
pub const INPUT_TOKENS: &str = "input_tokens";
/// Assistant output token count, when recorded.
pub const OUTPUT_TOKENS: &str = "output_tokens";

// Linear.
/// Linear issue identifier, e.g. `ENG-1885`.
pub const IDENTIFIER: &str = "identifier";
/// Linear team key, e.g. `ENG`.
pub const TEAM_KEY: &str = "team_key";
/// Stable Linear workflow-state type (`backlog`/`started`/`completed`/...).
pub const STATE_TYPE: &str = "state_type";
/// Linear assignee email.
pub const ASSIGNEE_EMAIL: &str = "assignee_email";
/// Linear labels.
pub const LABELS: &str = "labels";
/// Whether the issue is archived.
pub const IS_ARCHIVED: &str = "is_archived";
