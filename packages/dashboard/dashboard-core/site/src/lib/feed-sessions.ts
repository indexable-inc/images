import { paneScope } from './scope.ts';
import type { PaneRecord } from './types.ts';

export interface FeedSession {
  scope: string;
  label: string;
}

export function shortScope(scope: string): string {
  return scope ? scope.slice(0, 8) : 'local';
}

// Group panes into sessions by scope and resolve each session's label from its
// reserved `renderer:'session'` pane. Ordering is left to the caller (the
// sidebar sorts by last run activity); the scope sort here is only a stable,
// deterministic default so the set is reproducible.
export function feedSessions(panes: Record<string, PaneRecord>): FeedSession[] {
  const labels = new Map<string, string>();
  const scopes = new Set<string>();

  for (const [key, pane] of Object.entries(panes)) {
    const scope = paneScope(key);
    scopes.add(scope);
    if ((pane.kind ?? 'data') === 'data' && pane.renderer === 'session') {
      labels.set(scope, pane.title || 'session');
    }
  }

  return [...scopes]
    .map((scope) => ({
      scope,
      label: labels.get(scope) || shortScope(scope),
    }))
    .sort((a, b) => a.scope.localeCompare(b.scope));
}
