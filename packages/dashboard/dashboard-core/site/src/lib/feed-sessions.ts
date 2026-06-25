import { compareCreatedAt, paneScope } from './scope.ts';
import type { PaneRecord } from './types.ts';

export interface FeedSession {
  scope: string;
  label: string;
  createdAt?: number;
}

export function shortScope(scope: string): string {
  return scope ? scope.slice(0, 8) : 'local';
}

function noteCreated(created: Map<string, number>, scope: string, pane: PaneRecord): void {
  const value = pane.created_at;
  if (value === undefined) return;
  const previous = created.get(scope);
  if (previous === undefined || value < previous) created.set(scope, value);
}

function compareFeedSessions(a: FeedSession, b: FeedSession): number {
  const byCreated = compareCreatedAt(a.createdAt, b.createdAt);
  if (byCreated !== 0) return byCreated;
  return a.scope.localeCompare(b.scope);
}

export function feedSessions(panes: Record<string, PaneRecord>): FeedSession[] {
  const labels = new Map<string, string>();
  const created = new Map<string, number>();
  const scopes = new Set<string>();

  for (const [key, pane] of Object.entries(panes)) {
    const scope = paneScope(key);
    scopes.add(scope);
    noteCreated(created, scope, pane);
    if ((pane.kind ?? 'data') === 'data' && pane.renderer === 'session') {
      labels.set(scope, pane.title || 'session');
    }
  }

  return [...scopes]
    .map((scope) => ({
      scope,
      label: labels.get(scope) || shortScope(scope),
      createdAt: created.get(scope),
    }))
    .sort(compareFeedSessions);
}
