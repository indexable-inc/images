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
