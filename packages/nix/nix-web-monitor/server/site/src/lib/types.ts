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

/// Filesystem syscalls the nix-daemon made, grouped by class. Mirrors the Rust
/// `DaemonOps`; the daemon panel renders these as the breakdown of what kind of
/// work the daemon is doing (link/rename dominate store optimisation,
/// write/fsync dominate writing a new path).
export const daemonOpsSchema = v.object({
  link: v.number(),
  rename: v.number(),
  open: v.number(),
  write: v.number(),
  fsync: v.number(),
  stat: v.number(),
  unlink: v.number(),
  other: v.number()
});

/// One hot path sampled from daemon syscalls.
export const daemonHotPathSchema = v.object({
  path: v.string(),
  count: v.number(),
  opsPerSec: v.number()
});

/// Live nix-daemon syscall view. `tracing` is false when no tracer is attached
/// (no daemon, or it needs root), in which case `status` explains why and the
/// counters are zero. Mirrors the Rust `DaemonInfo`.
export const daemonInfoSchema = v.object({
  tracing: v.boolean(),
  status: v.string(),
  workers: v.array(v.number()),
  ops: daemonOpsSchema,
  opsPerSec: v.number(),
  currentPath: v.nullable(v.string()),
  hotPaths: v.array(daemonHotPathSchema)
});

/// Why one machine-wide build is happening: the chain from the requested root
/// derivation down to this goal, plus the cause that forced it. Mirrors the Rust
/// `GlobalWhy`; every field is optional so a source that omits one still parses.
export const globalWhySchema = v.object({
  rootDrvPath: v.nullable(v.string()),
  chain: v.array(v.string()),
  cause: v.nullable(v.string())
});

/// The kind of machine-wide goal. Mirrors the Rust `GlobalBuildKind`, which
/// already folds any unknown kind from the C++ side into `other` before it
/// reaches the wire, so the wire value is a closed set.
export const globalBuildKindSchema = v.picklist(['build', 'substitution', 'other']);

/// One active build or substitution goal on the machine, from the patched-nix
/// `nix store builds --json` subcommand. Mirrors the Rust `GlobalBuild`; a
/// substitution has a null `drvPath` and sets `storePath`. `startTime` is unix
/// *seconds* (the rest of the monitor uses milliseconds), so the panel multiplies
/// by 1000 before diffing against its clock.
export const globalBuildSchema = v.object({
  drvPath: v.nullable(v.string()),
  storePath: v.nullable(v.string()),
  outputs: v.array(v.string()),
  type: globalBuildKindSchema,
  pid: v.nullable(v.number()),
  startTime: v.nullable(v.number()),
  user: v.nullable(v.string()),
  uid: v.nullable(v.number()),
  logFile: v.nullable(v.string()),
  why: globalWhySchema
});

/// Machine-wide build view. `detected` is false on stock nix (the subcommand is
/// unavailable), in which case the panel hides and `status` explains why. Mirrors
/// the Rust `GlobalBuilds`.
export const globalBuildsSchema = v.object({
  detected: v.boolean(),
  builds: v.array(globalBuildSchema),
  status: v.string()
});

/// One activation step (a `home`/`os` switch's `activate` run): a named unit of
/// work plus the output lines it printed. Mirrors the Rust `ActivationStep`.
export const activationStepSchema = v.object({
  name: v.string(),
  status: v.picklist(['running', 'done', 'failed']),
  lines: v.array(v.string()),
  startedAtMs: v.number(),
  stoppedAtMs: v.nullable(v.number())
});

/// Live activation view, populated only during a switch. `active` is false on a
/// plain `nix build` (the panel hides); `status` mirrors `daemonInfo.status` as a
/// human line ("running", "skipped (build failed)", "done", "failed"). Mirrors
/// the Rust `Activation`.
export const activationSchema = v.object({
  active: v.boolean(),
  command: v.string(),
  steps: v.array(activationStepSchema),
  status: v.string()
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
  daemon: daemonInfoSchema,
  /// Machine-wide build view; `detected: false` on stock nix (panel hidden).
  global: globalBuildsSchema,
  /// Live activation view during a `home`/`os` switch; `active: false` otherwise.
  activation: activationSchema,
  /// Generation diff text (`nvd diff`), set once at the end of a switch.
  diff: v.nullable(v.string()),
  expected: v.record(v.string(), v.number()),
  dependencies: v.array(derivationEdgeSchema),
  /// Built derivations that are root *causes*: their whole input closure is
  /// cache hits, so their own source/inputs changed. Every other build is a
  /// forced cascade beneath one of these. Used to mark which builds are the
  /// actual triggers vs. dragged-along rebuilds.
  rootCauses: v.array(v.string()),
  /// "What changed" per root-cause derivation: derivation path -> human reason
  /// (e.g. "input rustc changed", "source changed", "no prior build to compare").
  /// Computed by the server diffing each root's `.drv` against its previous build.
  rebuildReasons: v.record(v.string(), v.string()),
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
  v.object({ type: v.literal('daemonSet'), daemon: daemonInfoSchema }),
  v.object({ type: v.literal('globalSet'), global: globalBuildsSchema }),
  v.object({ type: v.literal('activationSet'), activation: activationSchema }),
  v.object({ type: v.literal('diffSet'), diff: v.string() }),
  v.object({ type: v.literal('expectedSet'), name: v.string(), value: v.number() }),
  v.object({ type: v.literal('errorAppend'), message: v.string() }),
  v.object({ type: v.literal('dependenciesSet'), edges: v.array(derivationEdgeSchema) }),
  v.object({ type: v.literal('rootCausesSet'), derivations: v.array(v.string()) }),
  v.object({ type: v.literal('rebuildReasonSet'), derivation: v.string(), reason: v.string() }),
  v.object({ type: v.literal('finished'), exitCode: v.nullable(v.number()) })
]);

export type ActivityStatus = v.InferOutput<typeof activityStatusSchema>;
export type BuildStatus = v.InferOutput<typeof buildStatusSchema>;
export type ActivityType = v.InferOutput<typeof activityTypeSchema>;
export type ActivityProgress = v.InferOutput<typeof activityProgressSchema>;
export type OptimiseStats = v.InferOutput<typeof optimiseStatsSchema>;
export type DaemonOps = v.InferOutput<typeof daemonOpsSchema>;
export type DaemonHotPath = v.InferOutput<typeof daemonHotPathSchema>;
export type DaemonInfo = v.InferOutput<typeof daemonInfoSchema>;
export type GlobalWhy = v.InferOutput<typeof globalWhySchema>;
export type GlobalBuildKind = v.InferOutput<typeof globalBuildKindSchema>;
export type GlobalBuild = v.InferOutput<typeof globalBuildSchema>;
export type GlobalBuilds = v.InferOutput<typeof globalBuildsSchema>;
export type ActivationStep = v.InferOutput<typeof activationStepSchema>;
export type Activation = v.InferOutput<typeof activationSchema>;
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
  daemon: {
    tracing: false,
    status: '',
    workers: [],
      ops: { link: 0, rename: 0, open: 0, write: 0, fsync: 0, stat: 0, unlink: 0, other: 0 },
      opsPerSec: 0,
      currentPath: null,
      hotPaths: []
    },
  global: { detected: false, builds: [], status: '' },
  activation: { active: false, command: '', steps: [], status: '' },
  diff: null,
  expected: {},
  dependencies: [],
  rootCauses: [],
  rebuildReasons: {},
  exitCode: null,
  finished: false
});
