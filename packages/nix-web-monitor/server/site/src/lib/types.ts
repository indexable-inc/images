/// One source of truth: valibot schemas define every wire shape, types are
/// inferred from those schemas. Schemas double as runtime validators in
/// `monitor-store.ts`; types double as compile-time contracts everywhere
/// else. Add a field once (in the schema) and it shows up in the type.

import * as v from 'valibot';

export const activityStatusSchema = v.picklist(['running', 'stopped']);
export const buildStatusSchema = v.picklist(['running', 'stopped', 'succeeded', 'failed']);

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
  build: v.nullable(v.string())
});

export const buildNodeSchema = v.object({
  derivation: v.string(),
  activityId: v.nullable(v.number()),
  host: v.nullable(v.string()),
  phase: v.nullable(v.string()),
  status: buildStatusSchema,
  logCount: v.number(),
  startedAtMs: v.number(),
  stoppedAtMs: v.nullable(v.number())
});

export const logEntrySchema = v.object({
  index: v.number(),
  activityId: v.nullable(v.number()),
  /// Nix log level when known. 0=error, 1=warn, 2=notice, 3=info, 4+=debug.
  level: v.nullable(v.number()),
  text: v.string()
});

export const snapshotSchema = v.object({
  activities: v.array(activityNodeSchema),
  builds: v.array(buildNodeSchema),
  logs: v.array(logEntrySchema),
  errors: v.array(v.string()),
  progress: v.nullable(activityProgressSchema),
  expected: v.record(v.string(), v.number()),
  exitCode: v.nullable(v.number()),
  finished: v.boolean()
});

export type ActivityStatus = v.InferOutput<typeof activityStatusSchema>;
export type BuildStatus = v.InferOutput<typeof buildStatusSchema>;
export type ActivityType = v.InferOutput<typeof activityTypeSchema>;
export type ActivityProgress = v.InferOutput<typeof activityProgressSchema>;
export type ActivityNode = v.InferOutput<typeof activityNodeSchema>;
export type BuildNode = v.InferOutput<typeof buildNodeSchema>;
export type LogEntry = v.InferOutput<typeof logEntrySchema>;
export type MonitorSnapshot = v.InferOutput<typeof snapshotSchema>;

/// Purely client-side, never received over the wire.
export type ConnectionStatus = 'connecting' | 'live' | 'closed' | 'error';

/// Log-level filter choices for the log panel. Shared with the app shell so
/// the errors panel and keyboard shortcuts can drive the same filter state.
export const LOG_LEVEL_FILTERS = ['all', 'error', 'warn', 'info'] as const;
export type LogLevelFilter = (typeof LOG_LEVEL_FILTERS)[number];

/// Mirrors `activity_code::BUILD` in the parser; the protocol's name for an
/// individual derivation build activity.
export const ACTIVITY_NAME_BUILD = 'build';

export const EMPTY_SNAPSHOT: MonitorSnapshot = Object.freeze({
  activities: [],
  builds: [],
  logs: [],
  errors: [],
  progress: null,
  expected: {},
  exitCode: null,
  finished: false
});
