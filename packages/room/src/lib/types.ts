// Types mirroring the room server's wire format. Keep in lockstep with
// packages/room/src/db.rs and packages/room/src/events.rs. The server
// uses serde with `kebab-case` discriminators on ServerEvent.

export type ThreadStatus = 'active' | 'idle' | 'blocked' | 'archived';

export interface Thread {
  id: string;
  user: string;
  host: string;
  repo: string | null;
  branch: string | null;
  cwd: string | null;
  workspace_root?: string | null;
  base_sha?: string | null;
  title: string;
  status: ThreadStatus | string;
  model: string | null;
  reasoning_effort?: string | null;
  approval_policy?: unknown | null;
  permission_profile?: string | null;
  created_ms: number;
  updated_ms: number;
  message_count: number;
  preview: string;
  plan: ThreadPlan | null;
  goal: ThreadGoal | null;
}

export type PlanStepStatus = 'pending' | 'inProgress' | 'completed';

export interface PlanStep {
  step: string;
  status: PlanStepStatus;
}

export interface ThreadPlan {
  explanation: string | null;
  steps: PlanStep[];
}

/** Thread-scoped objective the user sets via `/goal` (Codex TUI) or
 *  the GoalPanel. Distinct from `ThreadPlan`: a plan is the agent's
 *  evolving per-turn TODO list, a goal is the stable user-set target
 *  the agent works against across turns, with optional token + time
 *  budgets that codex tracks itself. Mirrored from codex's
 *  `thread/goal/updated` notification. */
export interface ThreadGoal {
  objective: string;
  /** Codex status string. Observed values include "active";
   *  others (completed, exceeded, cleared) are accepted opaquely so
   *  the UI doesn't need to ship a release every time codex adds
   *  one. */
  status: string;
  tokenBudget: number | null;
  tokensUsed: number;
  timeUsedSeconds: number;
}

export type MessageRole = 'user' | 'assistant' | 'tool' | 'system';

export type MessageKind =
  | 'user_prompt'
  | 'assistant_text'
  | 'thinking'
  | 'tool_call'
  | 'tool_result'
  | 'subagent'
  | 'compact'
  | 'permission_request'
  | 'system'
  | 'error';

/** Typed outcome of a tool call. Mirrors the server's `ToolResult`
 *  (packages/room-server/src/tool_result.rs), a serde tagged union on
 *  `status`. Replaces the old lossy `tool_output: string | null`, which
 *  collapsed running, no-output, a real value, and an error into one
 *  string (a failed call's null result rendered as the text "null").
 *  `display` is the flattened text computed on the server; `content`
 *  keeps the structured payload for richer rendering. */
export type ToolResult =
  | { status: 'running'; display?: string | null }
  | {
      status: 'ok';
      display?: string | null;
      content?: unknown | null;
      exit_code?: number | null;
      duration_ms?: number | null;
    }
  | { status: 'empty'; duration_ms?: number | null }
  | {
      status: 'error';
      message: string;
      display?: string | null;
      content?: unknown | null;
      exit_code?: number | null;
      duration_ms?: number | null;
    }
  | { status: 'cancelled' };

export interface Message {
  id: string;
  thread_id: string;
  ts_ms: number;
  role: MessageRole | string;
  kind: MessageKind | string;
  text: string | null;
  tool_name: string | null;
  tool_use_id: string | null;
  tool_input: unknown | null;
  result: ToolResult | null;
  patch: string | null;
  /** Inline image attachments as `data:image/*;base64,…` URLs.
   *  Omitted by the server when empty (serde skips empty Vec), so
   *  treat both `undefined` and `[]` as "no attachments". */
  images?: string[];
}

export type ServerEvent =
  | { type: 'bootstrap'; threads: Thread[] }
  | { type: 'thread-upsert'; thread: Thread }
  | { type: 'message-append'; thread_id: string; message: Message }
  | { type: 'message-update'; thread_id: string; message: Message }
  | { type: 'thread-archive'; thread_id: string }
  | { type: 'ping' };
