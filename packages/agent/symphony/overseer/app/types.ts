// Shape of the JSON the tick script splices into the page (data.json).
export type AgentState = "progressing" | "waiting" | "stuck" | "idle";

export interface AgentJudgment {
  label: string;
  repo: string;
  doing: string;
  state: AgentState;
  why: string;
}

export interface AttentionItem {
  severity: "fix" | "watch";
  title: string;
  why: string;
  action: string;
  dispatched?: string | null;
}

export interface Report {
  digest: string;
  attention: AttentionItem[];
  agents: AgentJudgment[];
}

export interface HistoryPoint {
  ts: string;
  load: number;
  cpu: number;
  mem: number;
  sessions: number;
  stuck: number;
}

export interface RunInfo {
  run_id: string;
  status: string;
  trigger: string;
  updated_at: string;
}

export interface Data {
  generated_at: string;
  load_1m: number;
  ncpu: number;
  report: Report;
  history: HistoryPoint[];
  runs: RunInfo[];
  notes: string;
}
