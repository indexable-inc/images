export type ActivityStatus = 'running' | 'stopped';
export type BuildStatus = 'running' | 'succeeded' | 'failed';

export type ActivityType = {
  code: number;
  name: string;
};

export type ActivityProgress = {
  done: number;
  expected: number;
  running: number;
  failed: number;
};

export type ActivityNode = {
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
};

export type BuildNode = {
  derivation: string;
  activityId: number;
  host: string | null;
  phase: string | null;
  status: BuildStatus;
  logCount: number;
};

export type LogEntry = {
  index: number;
  activityId: number | null;
  text: string;
};

export type MonitorSnapshot = {
  activities: ActivityNode[];
  builds: BuildNode[];
  logs: LogEntry[];
  messages: string[];
  errors: string[];
  progress: ActivityProgress | null;
  expected: Record<string, number>;
  exitCode: number | null;
  finished: boolean;
};

export type ConnectionStatus = 'connecting' | 'live' | 'closed' | 'error';

export const EMPTY_SNAPSHOT: MonitorSnapshot = {
  activities: [],
  builds: [],
  logs: [],
  messages: [],
  errors: [],
  progress: null,
  expected: {},
  exitCode: null,
  finished: false
};
