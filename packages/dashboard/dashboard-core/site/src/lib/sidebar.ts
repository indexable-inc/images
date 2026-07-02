// The unified sidebar's data model: sessions (each holding its runs), resources,
// and recordings, grouped under foldable sections. Building the tree once — and
// reading it for both rendering and keyboard navigation — guarantees that what
// `j`/`k` walk is exactly what the eye sees, honouring every fold.
import { paneScope, compareCreatedAt } from './scope.ts';
import { feedSessions, shortScope, type FeedSession } from './feed-sessions.ts';
import { isRun, isResource, ledOf, kindOf, withKey, type Led } from './run.ts';
import type { Pane, PaneRecord } from './types.ts';
import type { RecordingInfo } from './stream.svelte.ts';

// A selectable target in the center stage: a run pane, a resource pane, or a
// recording. The key is the pane key (runs/resources) or the recording id.
export type Selection =
  | { kind: 'run'; key: string }
  | { kind: 'resource'; key: string }
  | { kind: 'recording'; id: string };

export function selectionEq(a: Selection | null, b: Selection | null): boolean {
  if (!a || !b) return a === b;
  if (a.kind !== b.kind) return false;
  return a.kind === 'recording' ? a.id === (b as { id: string }).id : a.key === (b as { key: string }).key;
}

// The three top-level sections. Their ids are the localStorage fold keys.
export type SectionId = 'sessions' | 'resources' | 'recordings';

export interface RunNode {
  key: string;
  pane: Pane;
  led: Led;
}

export interface SessionNode {
  scope: string;
  label: string;
  runs: RunNode[];
}

export interface ResourceNode {
  key: string;
  pane: Pane;
  led: Led;
}

export interface SidebarModel {
  sessions: SessionNode[];
  resources: ResourceNode[];
  recordings: RecordingInfo[];
  runCount: number;
}

function paneList(panes: Record<string, PaneRecord>): { key: string; pane: Pane }[] {
  return Object.keys(panes).map((key) => ({ key, pane: withKey(key, panes[key], paneScope(key)) }));
}

// Newest-first within a session (created_at desc, key as the stable tie-break).
function compareRunsNewestFirst(a: RunNode, b: RunNode): number {
  const byCreated = compareCreatedAt(b.pane.created_at, a.pane.created_at);
  if (byCreated !== 0) return byCreated;
  return a.key.localeCompare(b.key);
}

// Build the whole sidebar model from the live pane set plus the recordings list.
export function buildSidebar(
  panes: Record<string, PaneRecord>,
  recordings: RecordingInfo[],
): SidebarModel {
  const all = paneList(panes);

  // Sessions in first-appearance order (feedSessions is the shared grouping).
  const sessionOrder: FeedSession[] = feedSessions(panes);
  const runsByScope = new Map<string, RunNode[]>();
  for (const { key, pane } of all) {
    if (!isRun(key, pane)) continue;
    const rows = runsByScope.get(pane.scope) ?? [];
    rows.push({ key, pane, led: ledOf(pane) });
    runsByScope.set(pane.scope, rows);
  }

  // feedSessions lists every scope that has any pane; keep only those holding
  // runs, so a pure-resource scope doesn't show as an empty session.
  const sessions: SessionNode[] = sessionOrder
    .map((s) => ({
      scope: s.scope,
      label: s.label,
      runs: (runsByScope.get(s.scope) ?? []).sort(compareRunsNewestFirst),
    }))
    .filter((s) => s.runs.length > 0);

  const resources: ResourceNode[] = all
    .filter(({ key, pane }) => isResource(key, pane))
    .map(({ key, pane }) => ({ key, pane, led: ledOf(pane) }))
    .sort(
      (a, b) =>
        compareCreatedAt(a.pane.created_at, b.pane.created_at) || a.key.localeCompare(b.key),
    );

  const runCount = sessions.reduce((n, s) => n + s.runs.length, 0);

  return {
    sessions,
    resources,
    recordings: [...recordings].sort((a, b) => b.started_ms - a.started_ms),
    runCount,
  };
}

// A resource's right-aligned meta: a terminal's geometry, else its kind/subtitle.
export function resourceMeta(p: Pane): string {
  if (kindOf(p) === 'terminal') return `${p.rows ?? '?'}×${p.cols ?? '?'}`;
  return p.subtitle || p.kind || 'html';
}

// One flattened, currently-visible selectable row, in render order — exactly what
// `j`/`k` step through. Session headers and section headers are not selectable
// targets (folding them is a separate motion), so only runs, resources, and
// recordings appear here.
export interface FlatRow {
  selection: Selection;
}

// Walk the visible tree honouring fold state. `open` reports whether a fold key
// (a section id or a session scope prefixed `sess:`) is expanded.
export function flattenVisible(
  model: SidebarModel,
  open: (foldKey: string) => boolean,
): FlatRow[] {
  const out: FlatRow[] = [];
  if (open('sessions')) {
    for (const s of model.sessions) {
      if (!open('sess:' + s.scope)) continue;
      for (const r of s.runs) out.push({ selection: { kind: 'run', key: r.key } });
    }
  }
  if (open('resources')) {
    for (const r of model.resources) out.push({ selection: { kind: 'resource', key: r.key } });
  }
  if (open('recordings')) {
    for (const rec of model.recordings) out.push({ selection: { kind: 'recording', id: rec.id } });
  }
  return out;
}

export { shortScope };
