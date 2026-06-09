/// One source of truth: valibot schemas define every wire shape, types are
/// inferred from those schemas. Schemas double as runtime validators in
/// `monitor-store.ts`; types double as compile-time contracts everywhere
/// else. Add a field once (in the schema) and it shows up in the type.

import * as v from 'valibot';

export const activityStatusSchema = v.picklist(['running', 'stopped']);
/// `planned` is a derivation Nix announced in its build plan but has not started
/// yet. It seeds the tree up front so the target and its full subtree render
/// before any leaf begins, rather than the tree growing bottom-up from starts.
export const buildStatusSchema = v.picklist([
  'planned',
  'running',
  'stopped',
  'succeeded',
  'failed'
]);

export const activityTypeSchema = v.object({
  code: v.number(),
  name: v.string()
});

export const activityProgressSchema = v.object({
  done: v.number(),
  expected: v.number(),
  running: v.number(),
  failed: v.number()
});

/// Run-wide store-optimisation totals: how many duplicate store files Nix
/// replaced with hard links and the apparent bytes that reclaimed. Summed by
/// the parser from per-file `FileLinked` events; surfaced so the operator can
/// see the store-optimisation work behind an otherwise-silent slow store add.
export const optimiseStatsSchema = v.object({
  filesLinked: v.number(),
  bytesFreed: v.number()
});

export const activityNodeSchema = v.object({
  id: v.number(),
  parent: v.nullable(v.number()),
  activityType: activityTypeSchema,
  text: v.string(),
  phase: v.nullable(v.string()),
  progress: v.nullable(activityProgressSchema),
  status: activityStatusSchema,
  startedTick: v.number(),
  startedAtMs: v.number(),
  stoppedAtMs: v.nullable(v.number()),
  build: v.nullable(v.string()),
  /// Total bytes the server measured for a "copying … to the store" activity,
  /// which Nix reports without byte progress. `null` on every other activity.
  sizeBytes: v.nullable(v.number())
});

export const buildNodeSchema = v.object({
  derivation: v.string(),
  activityId: v.nullable(v.number()),
  host: v.nullable(v.string()),
  phase: v.nullable(v.string()),
  status: buildStatusSchema,
  logCount: v.number(),
  startedAtMs: v.number(),
  stoppedAtMs: v.nullable(v.number()),
  /// True when Nix resolved this derivation before building it (content-
  /// addressed). The row shows a `ca` badge and the resolved build is folded in.
  contentAddressed: v.boolean()
});

export const logEntrySchema = v.object({
  index: v.number(),
  activityId: v.nullable(v.number()),
  /// Nix log level when known. 0=error, 1=warn, 2=notice, 3=info, 4+=debug.
  level: v.nullable(v.number()),
  text: v.string()
});

/// One directed dependency edge in the build DAG: `from` directly requires
/// `to`. Both are `derivation` paths matching a `BuildNode`, so the tree view
/// joins edges to build rows by string identity.
export const derivationEdgeSchema = v.object({
  from: v.string(),
  to: v.string()
});

export const snapshotSchema = v.object({
  /// The Nix invocation being monitored, shown as the build tree's root label.
  command: v.string(),
  activities: v.array(activityNodeSchema),
  builds: v.array(buildNodeSchema),
  logs: v.array(logEntrySchema),
  errors: v.array(v.string()),
  progress: v.nullable(activityProgressSchema),
  optimise: optimiseStatsSchema,
  expected: v.record(v.string(), v.number()),
  dependencies: v.array(derivationEdgeSchema),
  exitCode: v.nullable(v.number()),
  finished: v.boolean()
});

/// One incremental change to the monitor state, mirroring Rust's `Delta` enum.
/// The live WebSocket stream carries these (one msgpack frame each) after an
/// initial `reset` seed; the discriminant rides in `type`. Field names are camelCase to
/// match the serde wire shape.
export const deltaSchema = v.variant('type', [
  v.object({ type: v.literal('reset'), snapshot: snapshotSchema }),
  v.object({ type: v.literal('buildUpsert'), build: buildNodeSchema }),
  v.object({ type: v.literal('activityUpsert'), activity: activityNodeSchema }),
  v.object({ type: v.literal('logsAppend'), entries: v.array(logEntrySchema) }),
  v.object({ type: v.literal('progressSet'), progress: activityProgressSchema }),
  v.object({ type: v.literal('optimiseSet'), optimise: optimiseStatsSchema }),
  v.object({ type: v.literal('expectedSet'), name: v.string(), value: v.number() }),
  v.object({ type: v.literal('errorAppend'), message: v.string() }),
  v.object({ type: v.literal('dependenciesSet'), edges: v.array(derivationEdgeSchema) }),
  v.object({ type: v.literal('finished'), exitCode: v.nullable(v.number()) })
]);

export type ActivityStatus = v.InferOutput<typeof activityStatusSchema>;
export type BuildStatus = v.InferOutput<typeof buildStatusSchema>;
export type ActivityType = v.InferOutput<typeof activityTypeSchema>;
export type ActivityProgress = v.InferOutput<typeof activityProgressSchema>;
export type OptimiseStats = v.InferOutput<typeof optimiseStatsSchema>;
export type ActivityNode = v.InferOutput<typeof activityNodeSchema>;
export type BuildNode = v.InferOutput<typeof buildNodeSchema>;
export type LogEntry = v.InferOutput<typeof logEntrySchema>;
export type DerivationEdge = v.InferOutput<typeof derivationEdgeSchema>;
export type MonitorSnapshot = v.InferOutput<typeof snapshotSchema>;
export type Delta = v.InferOutput<typeof deltaSchema>;

/// Purely client-side, never received over the wire. `reconnecting` is surfaced
/// only after a session has been down past the grace window, so a brief blip
/// that the client recovers from never flips the indicator off `live`.
export type ConnectionStatus = 'connecting' | 'live' | 'reconnecting' | 'closed' | 'error';

/// Log-level filter choices for the log panel. Shared with the app shell so
/// the errors panel and keyboard shortcuts can drive the same filter state.
export const LOG_LEVEL_FILTERS = ['all', 'error', 'warn', 'info'] as const;
export type LogLevelFilter = (typeof LOG_LEVEL_FILTERS)[number];

/// Mirrors `activity_code::BUILD` in the parser; the protocol's name for an
/// individual derivation build activity.
export const ACTIVITY_NAME_BUILD = 'build';

export const EMPTY_SNAPSHOT: MonitorSnapshot = Object.freeze({
  command: '',
  activities: [],
  builds: [],
  logs: [],
  errors: [],
  progress: null,
  optimise: { filesLinked: 0, bytesFreed: 0 },
  expected: {},
  dependencies: [],
  exitCode: null,
  finished: false
});
