// Liveness window for the "agent is working" indicator.
//
// thread.status flips to 'active' on every codex turn start, but is
// only flipped back to 'idle' when the bridge sees a `turn/completed`
// notification. If the codex subprocess crashes or the server restarts
// mid-turn, the row can stay 'active' forever — the spinner would
// then run indefinitely on a thread no live process is actually
// working on.
//
// The server resets stuck threads on startup and when the codex
// bridge disconnects, but the client also gates the spinner on
// recency as a belt-and-suspenders safety net for missed
// ThreadUpsert deltas, network reorderings, and any future stuck
// states that slip past the server. During a real turn deltas arrive
// every few seconds, so a 2-minute quiet window is generous.

import type { Thread } from './types';

export const AGENT_WORK_STALE_MS = 120_000;

export function isAgentWorkStale(thread: Thread, now: number): boolean {
  return now - thread.updated_ms > AGENT_WORK_STALE_MS;
}

export function agentWorkMode(
  thread: Thread | undefined,
  now: number
): 'working' | 'waiting' | null {
  if (!thread) return null;
  if (isAgentWorkStale(thread, now)) return null;
  if (thread.status === 'active') return 'working';
  if (thread.status === 'blocked') return 'waiting';
  return null;
}
