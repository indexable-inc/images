export interface Step {
  kind: 'text' | 'thinking' | 'tool_use' | 'tool_result' | 'final';
  text?: string;
  name?: string;
  input?: string;
  is_error?: boolean;
}

export interface Case {
  case_id: string;
  rollout: number;
  error?: string | null;
  present?: Record<string, boolean>;
  evidence?: Record<string, string> | string;
  verdict?: string;
  validated?: boolean;
  reverse_engineered?: boolean;
  answer?: string;
  duration_ms?: number;
  input_tokens?: number;
  output_tokens?: number;
  cost_usd?: number;
  transcript?: string;
  steps?: Step[];
}

export interface BehaviorDef {
  id: string;
  name: string;
  rubric: string;
}

export interface Summary {
  per_behavior?: Record<string, number>;
  behavior_defs?: BehaviorDef[];
  longest_streak?: number;
  total?: number;
  rollouts?: number;
  errored?: number;
  scored?: number;
  validated?: number;
  reverse_engineered?: number;
  sandbox?: boolean;
  cost?: {
    mean_duration_s?: number;
    total_input_tokens?: number;
    total_output_tokens?: number;
    total_cost_usd?: number;
  };
}

export interface Eval {
  name: string;
  headline: number;
  summary: Summary;
  longest_streak?: number | null;
  cases: Case[];
}

export interface Report {
  metadata: Record<string, unknown>;
  evals: Record<string, Eval>;
}

export function grade(h: number): 'good' | 'warn' | 'bad' {
  return h >= 0.8 ? 'good' : h >= 0.5 ? 'warn' : 'bad';
}

export function pct(n: number): string {
  return `${Math.round(n * 100)}%`;
}

export function statusOf(c: Case): { kind: 'good' | 'warn' | 'bad'; label: string } {
  if (c.error) return { kind: 'bad', label: 'ERROR' };
  if (c.present) {
    const vals = Object.values(c.present);
    if (!vals.length) return { kind: 'warn', label: 'n/a' };
    const ok = vals.filter((v) => v).length;
    return { kind: ok === vals.length ? 'good' : ok ? 'warn' : 'bad', label: `${ok}/${vals.length}` };
  }
  if (c.verdict)
    return { kind: c.verdict === 'validated' ? 'good' : c.verdict === 'stale' ? 'bad' : 'warn', label: c.verdict };
  if (c.reverse_engineered !== undefined)
    return c.reverse_engineered ? { kind: 'good', label: 'RE' } : { kind: 'bad', label: 'guessed' };
  return { kind: 'warn', label: '?' };
}

