-- Subagent cache schema (ENG-4665).
--
-- Owned and applied by the subagent-cache daemon on startup against its
-- dedicated internal Postgres. It is deliberately NOT part of the production
-- ix migration runner (crates/ix/db): that runner is bound to the global and
-- regional prod roles, and this cache lives in a separate database to keep its
-- blast radius isolated. Every statement is idempotent so bootstrap can run on
-- every start.

CREATE TABLE IF NOT EXISTS subagent_cache (
  id              uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
  agent_type      text        NOT NULL,          -- "explore" | "codebase-locator" | ...
  question        text        NOT NULL,          -- the prompt the subagent answered
  question_tsv    tsvector    GENERATED ALWAYS AS (to_tsvector('english', question)) STORED,
  findings        text        NOT NULL,          -- last_assistant_message (the cached value)
  agent_def_hash  text        NOT NULL,          -- xxh64 of .claude/agents/<type>.md
  model           text,                          -- generating model id, recorded for triage
  file_deps       jsonb       NOT NULL,          -- [{ "path": ..., "hash": "xxh64:<hex>" }]
  created_at      timestamptz NOT NULL DEFAULT now(),
  expires_at      timestamptz NOT NULL           -- TTL backstop
);

-- Stage-1 recall key: full-text rank over the question.
CREATE INDEX IF NOT EXISTS subagent_cache_question_tsv_idx
  ON subagent_cache USING gin (question_tsv);

-- Recall filter: same agent_type, matching persona, not expired.
CREATE INDEX IF NOT EXISTS subagent_cache_recall_idx
  ON subagent_cache (agent_type, agent_def_hash, expires_at);

-- One live row per (agent_type, question, persona): populate upserts onto it.
CREATE UNIQUE INDEX IF NOT EXISTS subagent_cache_upsert_key
  ON subagent_cache (agent_type, question, agent_def_hash);
