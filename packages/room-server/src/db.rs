// SQLite-backed storage for threads and messages.
//
// Schema is intentionally narrow: two tables, foreign-key cascade,
// indices that match the only query shapes the API exposes. We use
// `rusqlite` in bundled mode so deployments do not need a system libsqlite.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::tool_result::ToolResult;

pub struct Db {
    conn: Connection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub user: String,
    pub host: String,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub cwd: Option<String>,
    pub workspace_root: Option<String>,
    pub base_sha: Option<String>,
    pub title: String,
    pub status: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub approval_policy: Option<serde_json::Value>,
    pub permission_profile: Option<String>,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub message_count: i64,
    pub preview: String,
    pub plan: Option<ThreadPlan>,
    pub goal: Option<ThreadGoal>,
}

/// The agent's current TODO list for a thread, mirrored from codex's
/// `turn/plan/updated` notification. Each emission replaces the plan
/// in full, so we keep one row per thread.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadPlan {
    pub explanation: Option<String>,
    pub steps: Vec<PlanStep>,
}

/// The user-set objective for a thread, mirrored from codex's
/// `thread/goal/updated` notification. Goals are thread-scoped and
/// persist across turns; the agent tracks progress against the
/// optional token budget. Cleared by `thread/goal/cleared`.
///
/// Distinct from `ThreadPlan`: a plan is the agent's per-turn TODO
/// list, a goal is the user's stable target for the whole thread.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadGoal {
    pub objective: String,
    pub status: String,
    pub token_budget: Option<i64>,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanStep {
    pub step: String,
    pub status: PlanStepStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PlanStepStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub thread_id: String,
    pub ts_ms: i64,
    pub role: String,
    pub kind: String,
    pub text: Option<String>,
    pub tool_name: Option<String>,
    pub tool_use_id: Option<String>,
    pub tool_input: Option<serde_json::Value>,
    /// Typed outcome of a tool call (running / ok / empty / error /
    /// cancelled). Replaces the old lossy `tool_output: Option<String>`.
    /// `None` for non-tool messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ToolResult>,
    pub patch: Option<String>,
    /// Inline images attached to a user message, as data URLs
    /// (`data:image/png;base64,...`). Stored as a JSON array of
    /// strings in the `images_json` column. Empty for everything
    /// other than `user_prompt` messages today.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ThreadFilter {
    pub user: Option<String>,
    pub repo: Option<String>,
    pub status: Option<String>,
    pub search: Option<String>,
    pub limit: u32,
    pub before_updated_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ThreadUpsert {
    pub id: String,
    pub user: String,
    pub host: String,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub cwd: Option<String>,
    pub workspace_root: Option<String>,
    pub base_sha: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub approval_policy: Option<serde_json::Value>,
    pub permission_profile: Option<String>,
    pub title_if_empty: Option<String>,
    pub status: Option<String>,
    pub now_ms: i64,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Backend {
    pub id: String,
    pub name: String,
    pub url: String,
    pub source: String,
    pub run_id: Option<String>,
    pub node_id: Option<String>,
    pub vm_name: Option<String>,
    // The codex runtime that owns this backend ("host" or "ixvm"). The UI
    // renders it so an operator can see where a session is running.
    pub runtime: Option<String>,
    pub status: String,
    pub created_ms: i64,
    pub updated_ms: i64,
}

#[derive(Debug, Clone)]
pub struct BackendUpsert {
    pub id: String,
    pub name: String,
    pub url: String,
    pub source: String,
    pub run_id: Option<String>,
    pub node_id: Option<String>,
    pub vm_name: Option<String>,
    pub runtime: Option<String>,
    pub status: String,
    pub now_ms: i64,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create sqlite directory at {}", parent.display()))?;
        }
        let conn = Connection::open(path).context("open sqlite connection")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(Self { conn })
    }

    pub fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS threads (
                id              TEXT PRIMARY KEY,
                user            TEXT NOT NULL,
                host            TEXT NOT NULL,
                repo            TEXT,
                branch          TEXT,
                cwd             TEXT,
                title           TEXT NOT NULL DEFAULT 'Untitled',
                status          TEXT NOT NULL DEFAULT 'active',
                model           TEXT,
                created_ms      INTEGER NOT NULL,
                updated_ms      INTEGER NOT NULL,
                message_count   INTEGER NOT NULL DEFAULT 0,
                preview         TEXT NOT NULL DEFAULT ''
            );

            CREATE INDEX IF NOT EXISTS ix_threads_updated  ON threads (updated_ms DESC);
            CREATE INDEX IF NOT EXISTS ix_threads_user     ON threads (user);
            CREATE INDEX IF NOT EXISTS ix_threads_repo     ON threads (repo);
            CREATE INDEX IF NOT EXISTS ix_threads_status   ON threads (status);

            CREATE TABLE IF NOT EXISTS messages (
                id             TEXT PRIMARY KEY,
                thread_id      TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
                ts_ms          INTEGER NOT NULL,
                role           TEXT NOT NULL,
                kind           TEXT NOT NULL,
                text           TEXT,
                tool_name      TEXT,
                tool_use_id    TEXT,
                tool_input     TEXT,
                tool_output    TEXT,
                patch          TEXT
            );

            CREATE INDEX IF NOT EXISTS ix_messages_thread     ON messages (thread_id, ts_ms);
            CREATE INDEX IF NOT EXISTS ix_messages_tool_use   ON messages (thread_id, tool_use_id);

            -- Append-only log of every Loro CRDT update the server
            -- has accepted on the room socket. Rows are opaque bytes
            -- from the client; replay in `seq` order rebuilds the
            -- room's full presence + composer state. Kept separate
            -- from threads/messages because the CRDT carries
            -- ephemeral peer state, not user-authored content.
            CREATE TABLE IF NOT EXISTS loro_updates (
                seq    INTEGER PRIMARY KEY AUTOINCREMENT,
                ts_ms  INTEGER NOT NULL,
                bytes  BLOB    NOT NULL
            );

            -- Reviewer notes attached to agent-side messages. The
            -- source of truth is the Loro doc (one root LoroMap per
            -- message keyed `annotations:<message_id>`); this table
            -- is a mirror reconciled on every accepted Loro frame
            -- so retroactive AGENTS.md mining can `SELECT ... JOIN
            -- messages` without rehydrating the CRDT. `message_id`
            -- is NOT a foreign key — the annotated row may not exist
            -- yet at mirror time (Loro can outpace the message
            -- ingest path) and we'd rather over-store than drop.
            CREATE TABLE IF NOT EXISTS message_annotations (
                id           TEXT PRIMARY KEY,
                message_id   TEXT NOT NULL,
                author_id    TEXT NOT NULL,
                author_name  TEXT NOT NULL,
                ts_ms        INTEGER NOT NULL,
                text         TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS ix_message_annotations_message
                ON message_annotations (message_id);
            CREATE INDEX IF NOT EXISTS ix_message_annotations_ts
                ON message_annotations (ts_ms DESC);

            CREATE TABLE IF NOT EXISTS backends (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                url         TEXT NOT NULL,
                source      TEXT NOT NULL,
                run_id      TEXT,
                node_id     TEXT,
                vm_name     TEXT,
                runtime     TEXT,
                status      TEXT NOT NULL,
                created_ms  INTEGER NOT NULL,
                updated_ms  INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS ix_backends_status
                ON backends (status, updated_ms DESC);
            "#,
        )?;

        // SQLite has no `ADD COLUMN IF NOT EXISTS`, so probe first.
        self.add_column_if_missing("threads", "plan_json", "TEXT")?;
        self.add_column_if_missing("threads", "goal_json", "TEXT")?;
        self.add_column_if_missing("threads", "workspace_root", "TEXT")?;
        self.add_column_if_missing("threads", "base_sha", "TEXT")?;
        self.add_column_if_missing("threads", "reasoning_effort", "TEXT")?;
        self.add_column_if_missing("threads", "approval_policy_json", "TEXT")?;
        self.add_column_if_missing("threads", "permission_profile", "TEXT")?;
        self.add_column_if_missing("messages", "images_json", "TEXT")?;
        // Typed tool result (JSON of `ToolResult`). Supersedes the legacy
        // `tool_output` column, which is kept nullable so pre-migration
        // rows still read (see `row_to_message`).
        self.add_column_if_missing("messages", "result", "TEXT")?;
        self.add_column_if_missing("backends", "runtime", "TEXT")?;

        Ok(())
    }

    pub fn upsert_backend(&self, b: &BackendUpsert) -> Result<Backend> {
        let created_ms = self
            .get_backend(&b.id)?
            .map(|existing| existing.created_ms)
            .unwrap_or(b.now_ms);
        self.conn.execute(
            r#"
            INSERT INTO backends
              (id, name, url, source, run_id, node_id, vm_name, runtime, status, created_ms, updated_ms)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(id) DO UPDATE SET
                name       = excluded.name,
                url        = excluded.url,
                source     = excluded.source,
                run_id     = excluded.run_id,
                node_id    = excluded.node_id,
                vm_name    = excluded.vm_name,
                runtime    = excluded.runtime,
                status     = excluded.status,
                updated_ms = excluded.updated_ms
            "#,
            params![
                b.id, b.name, b.url, b.source, b.run_id, b.node_id, b.vm_name, b.runtime, b.status,
                created_ms, b.now_ms,
            ],
        )?;
        self.get_backend(&b.id)?
            .context("backend row vanished after upsert")
    }

    pub fn list_backends(&self) -> Result<Vec<Backend>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, url, source, run_id, node_id, vm_name, runtime, status, created_ms, updated_ms
               FROM backends
              WHERE status = 'active'
              ORDER BY updated_ms DESC",
        )?;
        stmt.query_map([], row_to_backend)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("list backends")
    }

    pub fn get_backend(&self, id: &str) -> Result<Option<Backend>> {
        self.conn
            .query_row(
                "SELECT id, name, url, source, run_id, node_id, vm_name, runtime, status, created_ms, updated_ms
                   FROM backends
                  WHERE id = ?1",
                params![id],
                row_to_backend,
            )
            .optional()
            .context("get backend")
    }

    pub fn delete_backend(&self, id: &str, now_ms: i64) -> Result<Option<Backend>> {
        let rows = self.conn.execute(
            "UPDATE backends SET status = 'inactive', updated_ms = ?2 WHERE id = ?1",
            params![id, now_ms],
        )?;
        if rows == 0 {
            return Ok(None);
        }
        self.get_backend(id)
    }

    fn add_column_if_missing(&self, table: &str, column: &str, ty: &str) -> Result<()> {
        let exists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
            params![table, column],
            |r| r.get(0),
        )?;
        if exists == 0 {
            // Table and column names are crate-local constants, not
            // user input, so the format-string SQL is safe.
            self.conn
                .execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {ty}"), [])?;
        }
        Ok(())
    }

    /// Create a minimal thread row if one does not already exist, so a
    /// streamed message can always satisfy the `messages.thread_id`
    /// foreign key. Engine-driven turns (`/api/agent/turns`) open a
    /// codex thread directly through the `Engine` trait, so the codex
    /// bridge sees `item/*` notifications for a thread the chat path
    /// never seeded; without this the first `insert_message` fails the
    /// FK and the whole transcript is dropped. `INSERT OR IGNORE` keeps
    /// it a no-op when a richer row already exists (chat path), so it is
    /// safe to call before every streamed write.
    pub fn ensure_thread(&self, id: &str, now_ms: i64) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO threads
               (id, user, host, title, status, created_ms, updated_ms, message_count, preview)
             VALUES (?1, '', '', 'Untitled', 'active', ?2, ?2, 0, '')",
            params![id, now_ms],
        )?;
        Ok(())
    }

    /// Insert-or-update a thread row in response to a hook event.
    /// Returns the row state after the upsert.
    pub fn upsert_thread(&self, u: &ThreadUpsert) -> Result<Thread> {
        let existing: Option<Thread> = self.get_thread(&u.id)?;
        let created_ms = existing.as_ref().map(|t| t.created_ms).unwrap_or(u.now_ms);
        let title = match (&existing, &u.title_if_empty) {
            (Some(t), _) if t.title != "Untitled" => t.title.clone(),
            (_, Some(t)) if !t.is_empty() => t.clone(),
            (Some(t), _) => t.title.clone(),
            (None, _) => "Untitled".to_owned(),
        };
        let status = u
            .status
            .clone()
            .or_else(|| existing.as_ref().map(|t| t.status.clone()))
            .unwrap_or_else(|| "active".to_owned());
        let preview = u
            .preview
            .clone()
            .or_else(|| existing.as_ref().map(|t| t.preview.clone()))
            .unwrap_or_default();
        let model = u
            .model
            .clone()
            .or_else(|| existing.as_ref().and_then(|t| t.model.clone()));
        let reasoning_effort = u
            .reasoning_effort
            .clone()
            .or_else(|| existing.as_ref().and_then(|t| t.reasoning_effort.clone()));
        let approval_policy = u
            .approval_policy
            .clone()
            .or_else(|| existing.as_ref().and_then(|t| t.approval_policy.clone()));
        let approval_policy_json = approval_policy
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .context("serialize approval policy")?;
        let permission_profile = u
            .permission_profile
            .clone()
            .or_else(|| existing.as_ref().and_then(|t| t.permission_profile.clone()));
        let repo = u
            .repo
            .clone()
            .or_else(|| existing.as_ref().and_then(|t| t.repo.clone()));
        let branch = u
            .branch
            .clone()
            .or_else(|| existing.as_ref().and_then(|t| t.branch.clone()));
        let cwd = u
            .cwd
            .clone()
            .or_else(|| existing.as_ref().and_then(|t| t.cwd.clone()));
        let workspace_root = u
            .workspace_root
            .clone()
            .or_else(|| existing.as_ref().and_then(|t| t.workspace_root.clone()));
        let base_sha = u
            .base_sha
            .clone()
            .or_else(|| existing.as_ref().and_then(|t| t.base_sha.clone()));
        let message_count = existing.as_ref().map(|t| t.message_count).unwrap_or(0);

        self.conn.execute(
            r#"
            INSERT INTO threads
              (id, user, host, repo, branch, cwd, title, status, model,
               created_ms, updated_ms, message_count, preview, workspace_root,
               base_sha, reasoning_effort, approval_policy_json, permission_profile)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
            ON CONFLICT(id) DO UPDATE SET
                repo        = COALESCE(excluded.repo, threads.repo),
                branch      = COALESCE(excluded.branch, threads.branch),
                cwd         = COALESCE(excluded.cwd, threads.cwd),
                workspace_root = COALESCE(excluded.workspace_root, threads.workspace_root),
                base_sha    = COALESCE(threads.base_sha, excluded.base_sha),
                model       = COALESCE(excluded.model, threads.model),
                reasoning_effort = COALESCE(excluded.reasoning_effort, threads.reasoning_effort),
                approval_policy_json = COALESCE(excluded.approval_policy_json, threads.approval_policy_json),
                permission_profile = COALESCE(excluded.permission_profile, threads.permission_profile),
                title       = CASE WHEN threads.title = 'Untitled' THEN excluded.title ELSE threads.title END,
                status      = excluded.status,
                updated_ms  = excluded.updated_ms,
                preview     = CASE WHEN excluded.preview != '' THEN excluded.preview ELSE threads.preview END
            "#,
            params![
                u.id,
                u.user,
                u.host,
                repo,
                branch,
                cwd,
                title,
                status,
                model,
                created_ms,
                u.now_ms,
                message_count,
                preview,
                workspace_root,
                base_sha,
                reasoning_effort,
                approval_policy_json,
                permission_profile,
            ],
        )?;

        self.get_thread(&u.id)?
            .context("thread row vanished after upsert")
    }

    pub fn get_thread(&self, id: &str) -> Result<Option<Thread>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, user, host, repo, branch, cwd, title, status, model,
                        created_ms, updated_ms, message_count, preview, plan_json,
                        goal_json, workspace_root, base_sha, reasoning_effort,
                        approval_policy_json, permission_profile
                 FROM threads WHERE id = ?1",
                params![id],
                row_to_thread,
            )
            .optional()?;
        Ok(row)
    }

    pub fn list_threads(&self, filter: &ThreadFilter) -> Result<Vec<Thread>> {
        let mut sql = String::from(
            "SELECT id, user, host, repo, branch, cwd, title, status, model,
                    created_ms, updated_ms, message_count, preview, plan_json,
                    goal_json, workspace_root, base_sha, reasoning_effort,
                    approval_policy_json, permission_profile
             FROM threads WHERE 1=1",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(u) = &filter.user {
            sql.push_str(" AND user = ?");
            args.push(Box::new(u.clone()));
        }
        if let Some(r) = &filter.repo {
            sql.push_str(" AND repo = ?");
            args.push(Box::new(r.clone()));
        }
        if let Some(s) = &filter.status {
            sql.push_str(" AND status = ?");
            args.push(Box::new(s.clone()));
        }
        if let Some(q) = &filter.search
            && !q.trim().is_empty()
        {
            sql.push_str(" AND (title LIKE ? OR preview LIKE ?)");
            let needle = format!("%{}%", q.trim());
            args.push(Box::new(needle.clone()));
            args.push(Box::new(needle));
        }
        if let Some(before) = filter.before_updated_ms {
            sql.push_str(" AND updated_ms < ?");
            args.push(Box::new(before));
        }
        sql.push_str(" ORDER BY updated_ms DESC LIMIT ?");
        let limit = if filter.limit == 0 {
            50
        } else {
            filter.limit.min(200)
        };
        args.push(Box::new(limit as i64));

        let params_dyn: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_dyn.as_slice(), row_to_thread)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn list_messages(&self, thread_id: &str, limit: u32) -> Result<Vec<Message>> {
        let limit = if limit == 0 { 500 } else { limit.min(2000) };
        let mut stmt = self.conn.prepare(
            "SELECT id, thread_id, ts_ms, role, kind, text, tool_name, tool_use_id,
                    tool_input, result, patch, images_json, tool_output
             FROM messages WHERE thread_id = ?1 ORDER BY ts_ms ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![thread_id, limit as i64], row_to_message)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn insert_message(&self, m: &Message) -> Result<Message> {
        let (msg, _) = self.upsert_message(m)?;
        Ok(msg)
    }

    /// Insert a message, or replace it in place if a row with the same
    /// id already exists. `message_count` only bumps on a true insert,
    /// so streaming callers can keep rewriting the same row as deltas
    /// arrive without inflating the thread's counter.
    pub fn upsert_message(&self, m: &Message) -> Result<(Message, bool)> {
        let tool_input = m
            .tool_input
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_default());
        let result_json = m
            .result
            .as_ref()
            .map(|r| serde_json::to_string(r).unwrap_or_default());
        let images_json = if m.images.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&m.images).unwrap_or_else(|_| "[]".to_owned()))
        };

        let existed: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM messages WHERE id = ?1",
                params![m.id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);

        self.conn.execute(
            "INSERT OR REPLACE INTO messages
              (id, thread_id, ts_ms, role, kind, text, tool_name, tool_use_id,
               tool_input, result, patch, images_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                m.id,
                m.thread_id,
                m.ts_ms,
                m.role,
                m.kind,
                m.text,
                m.tool_name,
                m.tool_use_id,
                tool_input,
                result_json,
                m.patch,
                images_json,
            ],
        )?;

        let count_delta = if existed { 0 } else { 1 };
        self.conn.execute(
            "UPDATE threads
                SET message_count = message_count + ?3,
                    updated_ms = MAX(updated_ms, ?2)
              WHERE id = ?1",
            params![m.thread_id, m.ts_ms, count_delta],
        )?;
        if let Some(text) = &m.text
            && m.role == "user"
        {
            let p = preview(text);
            if !p.is_empty() {
                self.conn.execute(
                    "UPDATE threads SET preview = ?2 WHERE id = ?1",
                    params![m.thread_id, p],
                )?;
            }
        }
        Ok((m.clone(), !existed))
    }

    /// Update an earlier tool_call message (matched by tool_use_id) with
    /// its typed result and optional patch. Used by PostToolUse hooks.
    /// Returns the message id that was updated, when found.
    pub fn update_tool_call(
        &self,
        thread_id: &str,
        tool_use_id: &str,
        result: Option<&ToolResult>,
        patch: Option<&str>,
    ) -> Result<Option<String>> {
        let id = self
            .conn
            .query_row(
                "SELECT id FROM messages
                 WHERE thread_id = ?1 AND tool_use_id = ?2 AND kind = 'tool_call'
                 ORDER BY ts_ms DESC LIMIT 1",
                params![thread_id, tool_use_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        if let Some(id) = id.as_deref() {
            let result_json = result.map(|r| serde_json::to_string(r).unwrap_or_default());
            self.conn.execute(
                "UPDATE messages SET result = ?2, patch = COALESCE(?3, patch) WHERE id = ?1",
                params![id, result_json, patch],
            )?;
        }
        Ok(id)
    }

    pub fn set_thread_title_if_default(&self, id: &str, title: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE threads SET title = ?2
             WHERE id = ?1 AND (title = 'Untitled' OR title = '')",
            params![id, title],
        )?;
        Ok(())
    }

    /// Replace a thread's plan in full. `plan = None` clears it.
    /// `updated_ms` bumps so the sidebar re-sorts the row when the
    /// agent updates its TODO list. Returns the updated row, or None
    /// when the thread does not exist.
    pub fn set_thread_plan(
        &self,
        id: &str,
        plan: Option<&ThreadPlan>,
        now_ms: i64,
    ) -> Result<Option<Thread>> {
        let json = plan
            .map(serde_json::to_string)
            .transpose()
            .context("serialize thread plan")?;
        let rows = self.conn.execute(
            "UPDATE threads SET plan_json = ?2, updated_ms = MAX(updated_ms, ?3) WHERE id = ?1",
            params![id, json, now_ms],
        )?;
        if rows == 0 {
            return Ok(None);
        }
        self.get_thread(id)
    }

    /// Replace a thread's goal in full. `goal = None` clears it.
    /// `updated_ms` bumps so the sidebar re-sorts. Returns the
    /// updated row, or None when the thread does not exist.
    pub fn set_thread_goal(
        &self,
        id: &str,
        goal: Option<&ThreadGoal>,
        now_ms: i64,
    ) -> Result<Option<Thread>> {
        let json = goal
            .map(serde_json::to_string)
            .transpose()
            .context("serialize thread goal")?;
        let rows = self.conn.execute(
            "UPDATE threads SET goal_json = ?2, updated_ms = MAX(updated_ms, ?3) WHERE id = ?1",
            params![id, json, now_ms],
        )?;
        if rows == 0 {
            return Ok(None);
        }
        self.get_thread(id)
    }

    /// Set a thread's status. Bumps updated_ms so the row re-sorts to
    /// the head of its section. Returns the updated row, or None when
    /// the thread does not exist.
    pub fn set_thread_status(&self, id: &str, status: &str, now_ms: i64) -> Result<Option<Thread>> {
        let rows = self.conn.execute(
            "UPDATE threads SET status = ?2, updated_ms = ?3 WHERE id = ?1",
            params![id, status, now_ms],
        )?;
        if rows == 0 {
            return Ok(None);
        }
        self.get_thread(id)
    }

    /// Flip every thread currently in a working / blocked status back
    /// to 'idle'. Used at server startup and whenever the codex
    /// subprocess disappears: without this, stale `active` rows leave
    /// the UI spinning forever on threads that no live process is
    /// actually working on. Returns the affected thread rows so the
    /// caller can broadcast ThreadUpsert deltas.
    pub fn reset_stuck_threads(&self) -> Result<Vec<Thread>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM threads WHERE status IN ('active', 'blocked')")?;
        let ids: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        self.conn.execute(
            "UPDATE threads SET status = 'idle' WHERE status IN ('active', 'blocked')",
            [],
        )?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(t) = self.get_thread(&id)? {
                out.push(t);
            }
        }
        Ok(out)
    }

    /// Append one Loro CRDT update frame to the durable log. The
    /// returned `seq` is the assigned row id; callers don't need it
    /// today but it's the natural handle for debug queries.
    pub fn append_loro_update(&self, ts_ms: i64, bytes: &[u8]) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO loro_updates (ts_ms, bytes) VALUES (?1, ?2)",
            params![ts_ms, bytes],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Load every persisted Loro update in apply order. Used at boot
    /// to rebuild the server-side `LoroDoc` before the first client
    /// connects. The log is opaque blobs — the caller imports them
    /// into a `LoroDoc` to materialize the state.
    pub fn all_loro_updates(&self) -> Result<Vec<Vec<u8>>> {
        let mut stmt = self
            .conn
            .prepare("SELECT bytes FROM loro_updates ORDER BY seq ASC")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, Vec<u8>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Metadata-only listing for the debug endpoint. Returns the
    /// most recent `limit` rows newest-first, without the blob
    /// payload — `/api/loro/updates` is for "did the log advance"
    /// sanity checks, not for re-shipping every frame over HTTP.
    pub fn recent_loro_update_meta(&self, limit: u32) -> Result<Vec<LoroUpdateMeta>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, ts_ms, length(bytes) FROM loro_updates ORDER BY seq DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit], |r| {
                Ok(LoroUpdateMeta {
                    seq: r.get(0)?,
                    ts_ms: r.get(1)?,
                    byte_len: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LoroUpdateMeta {
    pub seq: i64,
    pub ts_ms: i64,
    pub byte_len: i64,
}

/// One reviewer note pulled from the Loro mirror table. The fields
/// match the JSON the JS client encodes into each annotation map
/// value; see `Annotation` in `packages/room/src/lib/loro.ts`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Annotation {
    pub id: String,
    pub message_id: String,
    pub author_id: String,
    pub author_name: String,
    pub ts_ms: i64,
    pub text: String,
}

impl Db {
    /// Replace the SQL mirror for one annotated message with the
    /// caller-provided set. Used by the Loro reconcile pass: walk
    /// every `annotations:<message_id>` root map, pass its current
    /// entries here, and the mirror catches up in one transaction.
    /// Reconciling per-message (not globally) keeps the cost
    /// proportional to what just changed.
    pub fn reconcile_annotations_for(
        &mut self,
        message_id: &str,
        annotations: &[Annotation],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM message_annotations WHERE message_id = ?1",
            params![message_id],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO message_annotations
                   (id, message_id, author_id, author_name, ts_ms, text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for a in annotations {
                if a.message_id != message_id {
                    continue;
                }
                stmt.execute(params![
                    a.id,
                    a.message_id,
                    a.author_id,
                    a.author_name,
                    a.ts_ms,
                    a.text,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_annotations(&self, limit: u32) -> Result<Vec<Annotation>> {
        let limit = if limit == 0 { 200 } else { limit.min(1000) };
        let mut stmt = self.conn.prepare(
            "SELECT id, message_id, author_id, author_name, ts_ms, text
               FROM message_annotations
              ORDER BY ts_ms DESC
              LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok(Annotation {
                    id: r.get(0)?,
                    message_id: r.get(1)?,
                    author_id: r.get(2)?,
                    author_name: r.get(3)?,
                    ts_ms: r.get(4)?,
                    text: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

fn preview(s: &str) -> String {
    let trimmed = s.trim();
    let one_line = trimmed.lines().next().unwrap_or("");
    truncate_chars(one_line, 160)
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    let truncated: String = s.chars().take(max_chars).collect();
    format!("{truncated}...")
}

fn row_to_thread(row: &rusqlite::Row<'_>) -> rusqlite::Result<Thread> {
    let plan_json: Option<String> = row.get(13)?;
    let plan = plan_json.and_then(|s| serde_json::from_str::<ThreadPlan>(&s).ok());
    let goal_json: Option<String> = row.get(14)?;
    let goal = goal_json.and_then(|s| serde_json::from_str::<ThreadGoal>(&s).ok());
    let approval_policy_json: Option<String> = row.get(18)?;
    let approval_policy =
        approval_policy_json.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    Ok(Thread {
        id: row.get(0)?,
        user: row.get(1)?,
        host: row.get(2)?,
        repo: row.get(3)?,
        branch: row.get(4)?,
        cwd: row.get(5)?,
        workspace_root: row.get(15)?,
        base_sha: row.get(16)?,
        title: row.get(6)?,
        status: row.get(7)?,
        model: row.get(8)?,
        reasoning_effort: row.get(17)?,
        approval_policy,
        permission_profile: row.get(19)?,
        created_ms: row.get(9)?,
        updated_ms: row.get(10)?,
        message_count: row.get(11)?,
        preview: row.get(12)?,
        plan,
        goal,
    })
}

fn row_to_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let tool_input: Option<String> = row.get(8)?;
    let result_json: Option<String> = row.get(9)?;
    let images_json: Option<String> = row.get(11)?;
    let legacy_tool_output: Option<String> = row.get(12)?;
    let images = images_json
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default();
    // Prefer the typed `result` column; fall back to deriving one from a
    // pre-migration `tool_output` row so old transcripts still render.
    let result = result_json
        .and_then(|s| serde_json::from_str::<ToolResult>(&s).ok())
        .or_else(|| ToolResult::from_legacy(legacy_tool_output));
    Ok(Message {
        id: row.get(0)?,
        thread_id: row.get(1)?,
        ts_ms: row.get(2)?,
        role: row.get(3)?,
        kind: row.get(4)?,
        text: row.get(5)?,
        tool_name: row.get(6)?,
        tool_use_id: row.get(7)?,
        tool_input: tool_input.and_then(|s| serde_json::from_str(&s).ok()),
        result,
        patch: row.get(10)?,
        images,
    })
}

fn row_to_backend(row: &rusqlite::Row<'_>) -> rusqlite::Result<Backend> {
    Ok(Backend {
        id: row.get(0)?,
        name: row.get(1)?,
        url: row.get(2)?,
        source: row.get(3)?,
        run_id: row.get(4)?,
        node_id: row.get(5)?,
        vm_name: row.get(6)?,
        runtime: row.get(7)?,
        status: row.get(8)?,
        created_ms: row.get(9)?,
        updated_ms: row.get(10)?,
    })
}

pub fn derive_title(prompt: &str) -> String {
    let first = prompt.trim().lines().next().unwrap_or("").trim();
    if first.is_empty() {
        "Untitled".to_owned()
    } else {
        truncate_chars(first, 80)
    }
}

pub fn derive_preview(s: &str) -> String {
    preview(s)
}
