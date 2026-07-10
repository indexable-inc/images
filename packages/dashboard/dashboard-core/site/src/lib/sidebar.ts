// The unified sidebar's data model: sessions (each holding resources and runs),
// global resources, and recordings, grouped under foldable sections. Building
// the tree once and reading it for both rendering and keyboard navigation
// guarantees that what
// `j`/`k` walk is exactly what the eye sees, honouring every fold.
import { paneScope, compareCreatedAt } from './scope.ts';
import { feedSessions, shortScope, type FeedSession } from './feed-sessions.ts';
import { isRun, isResource, ledOf, kindOf, paneId, withKey, type Led } from './run.ts';
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
  resources: ResourceNode[];
}

export interface ResourceNode {
  key: string;
  pane: Pane;
  led: Led;
  parent?: string;
}

export interface TopicNode {
  key: string;
  label: string;
  runs: RunNode[];
  lastActivity?: number;
}

export interface SessionNode {
  scope: string;
  label: string;
  // The session's newest run's created_at; the card renders it as an age and the
  // sidebar sorts sessions by it (newest first).
  lastActivity?: number;
  resources: ResourceNode[];
  topics: TopicNode[];
  runs: RunNode[];
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

// Oldest-first within a session (created_at asc, key as the stable tie-break):
// the run list reads as a log that grows downward, so a new run appends at the
// bottom instead of pushing everything else down from the top.
function compareRunsOldestFirst(a: RunNode, b: RunNode): number {
  const byCreated = compareCreatedAt(a.pane.created_at, b.pane.created_at);
  if (byCreated !== 0) return byCreated;
  return a.key.localeCompare(b.key);
}

// A session's recency: the newest run it holds (max created_at over runs, not
// resource/namespace/output panes), so a session sorts and reads by when its
// last tool call ran. Runs are the isRun-filtered set, already sorted
// oldest-first, so the last one is the newest.
function lastRunActivity(runs: RunNode[]): number | undefined {
  return runs.length ? runs[runs.length - 1].pane.created_at : undefined;
}

function topicLabel(run: RunNode): string {
  const raw = run.pane.topic?.trim();
  return raw || 'unfiled';
}

function groupRunsByTopic(runs: RunNode[]): TopicNode[] {
  const topics: TopicNode[] = [];
  const byKey = new Map<string, TopicNode>();
  for (const run of runs) {
    const label = topicLabel(run);
    let topic = byKey.get(label);
    if (!topic) {
      topic = { key: label, label, runs: [], lastActivity: undefined };
      byKey.set(label, topic);
      topics.push(topic);
    }
    topic.runs.push(run);
    topic.lastActivity = run.pane.created_at;
  }
  return topics;
}

// Newest-activity first (a session whose last run is more recent sorts above one
// whose last run is older), scope as the stable tie-break. A session with no
// timed run sorts last.
function compareSessionsNewestFirst(a: SessionNode, b: SessionNode): number {
  const at = a.lastActivity;
  const bt = b.lastActivity;
  if (at !== undefined && bt !== undefined && at !== bt) return bt - at;
  if (at !== undefined && bt === undefined) return -1;
  if (at === undefined && bt !== undefined) return 1;
  return a.scope.localeCompare(b.scope);
}

// Build the whole sidebar model from the live pane set plus the recordings list.
export function buildSidebar(
  panes: Record<string, PaneRecord>,
  recordings: RecordingInfo[],
): SidebarModel {
  const all = paneList(panes);

  // feedSessions groups every scope and resolves its label; the sidebar owns the
  // ordering (by last run activity), so it is taken here, not there.
  const sessionOrder: FeedSession[] = feedSessions(panes);
  const runsByScope = new Map<string, RunNode[]>();
  const resourcesByScope = new Map<string, ResourceNode[]>();
  for (const { key, pane } of all) {
    if (isRun(key, pane)) {
      const rows = runsByScope.get(pane.scope) ?? [];
      rows.push({ key, pane, led: ledOf(pane), resources: [] });
      runsByScope.set(pane.scope, rows);
    } else if (isResource(key, pane)) {
      const rows = resourcesByScope.get(pane.scope) ?? [];
      rows.push({ key, pane, led: ledOf(pane), parent: pane.parent });
      resourcesByScope.set(pane.scope, rows);
    }
  }

  // Keep only scopes that hold runs (a pure-resource scope isn't a session),
  // then order sessions newest-run-first so the one you're actively using floats
  // to the top. Runs within a session stay oldest-first (a log growing downward).
  const sessions: SessionNode[] = sessionOrder
    .map((s) => {
      const runs = (runsByScope.get(s.scope) ?? []).sort(compareRunsOldestFirst);
      const allResources = (resourcesByScope.get(s.scope) ?? []).sort(
        (a, b) =>
          compareCreatedAt(a.pane.created_at, b.pane.created_at) || a.key.localeCompare(b.key),
      );
      const runsByPaneId = new Map(runs.map((run) => [paneId(run.key), run]));
      for (const resource of allResources) {
        const parent = resource.parent ? runsByPaneId.get(resource.parent) : undefined;
        if (parent) parent.resources.push(resource);
      }
      const resources = allResources.filter((resource) => !resource.parent || !runsByPaneId.has(resource.parent));
      return {
        scope: s.scope,
        label: s.label,
        lastActivity: lastRunActivity(runs),
        resources,
        topics: groupRunsByTopic(runs),
        runs,
      };
    })
    .filter((s) => s.runs.length > 0)
    .sort(compareSessionsNewestFirst);

  const resources: ResourceNode[] = all
    .filter(({ key, pane }) => isResource(key, pane))
    .map(({ key, pane }) => ({ key, pane, led: ledOf(pane), parent: pane.parent }))
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

// One flattened, currently-visible selectable row in render order, exactly what
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
      for (const r of s.resources) out.push({ selection: { kind: 'resource', key: r.key } });
      for (const t of s.topics) {
        if (!open('topic:' + s.scope + ':' + t.key)) continue;
        for (const r of t.runs) {
          out.push({ selection: { kind: 'run', key: r.key } });
          for (const resource of r.resources) out.push({ selection: { kind: 'resource', key: resource.key } });
        }
      }
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
