export type ActivityStatus = 'running' | 'stopped';
export type BuildStatus = 'running' | 'stopped' | 'succeeded' | 'failed';

export type ActivityType = Readonly<{
  code: number;
  name: string;
}>;

export type ActivityProgress = Readonly<{
  done: number;
  expected: number;
  running: number;
  failed: number;
}>;

export type ActivityNode = Readonly<{
  id: number;
  parent: number | null;
  activityType: ActivityType;
  text: string;
  phase: string | null;
  progress: ActivityProgress | null;
  status: ActivityStatus;
  startedTick: number;
  stoppedTick: number | null;
  build: string | null;
}>;

export type BuildNode = Readonly<{
  derivation: string;
  activityId: number | null;
  host: string | null;
  phase: string | null;
  status: BuildStatus;
  logCount: number;
}>;

export type LogEntry = Readonly<{
  index: number;
  activityId: number | null;
  /// Nix log level when known. 0=error, 1=warn, 2=notice, 3=info, 4+=debug-ish.
  level: number | null;
  text: string;
}>;

export type MonitorSnapshot = Readonly<{
  activities: ReadonlyArray<ActivityNode>;
  builds: ReadonlyArray<BuildNode>;
  logs: ReadonlyArray<LogEntry>;
  messages: ReadonlyArray<string>;
  errors: ReadonlyArray<string>;
  progress: ActivityProgress | null;
  expected: Readonly<Record<string, number>>;
  exitCode: number | null;
  finished: boolean;
}>;

export type ConnectionStatus = 'connecting' | 'live' | 'closed' | 'error';

/// Mirrors `activity_code::BUILD` in the parser; the protocol's name for an
/// individual derivation build activity.
export const ACTIVITY_NAME_BUILD = 'build';

export const EMPTY_SNAPSHOT: MonitorSnapshot = Object.freeze({
  activities: Object.freeze([]),
  builds: Object.freeze([]),
  logs: Object.freeze([]),
  messages: Object.freeze([]),
  errors: Object.freeze([]),
  progress: null,
  expected: Object.freeze({}),
  exitCode: null,
  finished: false
});
