import { compareCreatedAt, paneScope } from './scope.ts';
import type { Pane, PaneRecord } from './types.ts';

export interface NamespaceSession {
  key: string;
  pane: Pane;
}

function compareNamespaceSessions(a: NamespaceSession, b: NamespaceSession): number {
  const byCreated = compareCreatedAt(a.pane.created_at, b.pane.created_at);
  if (byCreated !== 0) return byCreated;
  return a.key.localeCompare(b.key);
}

export function namespaceSessions(panes: Record<string, PaneRecord>): NamespaceSession[] {
  return Object.keys(panes)
    .map((key) => ({
      key,
      pane: { ...panes[key], key, scope: paneScope(key) } as Pane,
    }))
    .filter((it) => (it.pane.kind ?? 'data') === 'data' && it.pane.renderer === 'namespace')
    .sort(compareNamespaceSessions);
}
