// Shared run/pane classification. One implementation of "what kind of thing is
// this pane and what state is it in", used by the sidebar (LED colour + grouping)
// and the run detail. Kept free of DOM/ansi imports so it is unit testable in
// plain node; the exec-stream parsers live in exec.ts.
import type { Pane, PaneRecord } from './types.ts';
import { SCOPE_SEP } from './scope.ts';

// A pane's kind, defaulting to 'data' (the wire omits it for plain data panes).
export function kindOf(p: PaneRecord): string {
  return p.kind ?? 'data';
}

// The three LED states a run/resource can be in. Success and idle share the
// hollow-ring "ok"; only a live run (amber pulse) and a failure (red) get a
// filled dot, so a column of "fine" doesn't compete for the eye.
export type Led = 'ok' | 'running' | 'error';

export function ledOf(p: PaneRecord): Led {
  const kind = kindOf(p);
  if (kind === 'exec') {
    if (p.running === true) return 'running';
    if (p.ok === false) return 'error';
    return 'ok';
  }
  if (kind === 'terminal') return p.alive === false ? 'error' : 'ok';
  return 'ok';
}

// The id half of a `scope<0x1f>id` pane key.
export function paneId(key: string): string {
  const sep = key.indexOf(SCOPE_SEP);
  return sep === -1 ? key : key.slice(sep + 1);
}

// A run's rich output is published as a sibling `<id>/out` pane; it is an
// attachment folded into the run's detail, never its own entry.
export function isOutputAttachment(key: string): boolean {
  return paneId(key).endsWith('/out');
}

// The reserved per-session label pane (renderer:'session'): carries a session's
// identity, not a run.
export function isSessionPane(p: PaneRecord): boolean {
  return kindOf(p) === 'data' && p.renderer === 'session';
}

// The kernel's live-globals pane (renderer:'namespace'): the right-rail
// inspector reads it; it is never a run.
export function isNamespacePane(p: PaneRecord): boolean {
  return kindOf(p) === 'data' && p.renderer === 'namespace';
}

// A resource is a long-lived interactive surface: a terminal, or an html pane
// keyed `resource/<id>` (a browser/vm the kernel publishes).
export function isResource(key: string, p: PaneRecord): boolean {
  if (kindOf(p) === 'terminal') return true;
  return paneId(key).startsWith('resource/');
}

// A run is any pane that belongs in a session's run list: an exec, or a plain
// data/html pane that is neither a session label, a namespace, a resource, nor a
// rich-output attachment.
export function isRun(key: string, p: PaneRecord): boolean {
  if (isSessionPane(p) || isNamespacePane(p)) return false;
  if (isResource(key, p)) return false;
  if (isOutputAttachment(key)) return false;
  return true;
}

// A pane record enriched with its key and producer scope.
export function withKey(key: string, record: PaneRecord, scope: string): Pane {
  return { ...record, key, scope };
}
