// Local-only draft threads.
//
// A draft is a thread that exists only in this client until the user
// sends the first message. Drafts show up in the sidebar with a "Draft"
// badge, take part in ⌘1..9 / ⌘[ / ⌘] navigation, and are discarded
// when the user dismisses them or sends successfully (the server then
// broadcasts the real thread under the same id).

import { writable, get, type Readable, type Writable } from 'svelte/store';
import type { Thread } from './types';
import { loadIdentity } from './identity';

export interface Draft {
  id: string;
  server_id: string;
  text: string;
  created_ms: number;
  updated_ms: number;
}

const draftsMap: Writable<Map<string, Draft>> = writable(new Map());

export const drafts: Readable<Map<string, Draft>> = { subscribe: draftsMap.subscribe };

export function createDraft(serverId: string): string {
  const id = crypto.randomUUID();
  const now = Date.now();
  draftsMap.update((m) => {
    m.set(id, { id, server_id: serverId, text: '', created_ms: now, updated_ms: now });
    return new Map(m);
  });
  return id;
}

export function getDraft(id: string): Draft | undefined {
  return get(draftsMap).get(id);
}

export function updateDraftText(id: string, text: string): void {
  draftsMap.update((m) => {
    const d = m.get(id);
    if (!d) return m;
    m.set(id, { ...d, text, updated_ms: Date.now() });
    return new Map(m);
  });
}

export function discardDraft(id: string): void {
  draftsMap.update((m) => {
    if (!m.has(id)) return m;
    m.delete(id);
    return new Map(m);
  });
}

export function isDraft(id: string): boolean {
  return get(draftsMap).has(id);
}

// Status sentinel surfaced via the standard Thread shape so existing
// renderers can switch on it without learning a new type.
export const DRAFT_STATUS = 'draft';

export function draftAsThread(d: Draft, serverName: string): Thread & { server_id: string; server_name: string } {
  const self = loadIdentity();
  const firstLine = d.text.split('\n')[0]?.trim() ?? '';
  return {
    id: d.id,
    server_id: d.server_id,
    server_name: serverName,
    user: self.name,
    host: '',
    repo: null,
    branch: null,
    cwd: null,
    workspace_root: null,
    base_sha: null,
    title: firstLine || 'Draft',
    status: DRAFT_STATUS,
    model: null,
    reasoning_effort: null,
    approval_policy: null,
    permission_profile: null,
    created_ms: d.created_ms,
    updated_ms: d.updated_ms,
    message_count: 0,
    preview: '',
    plan: null,
    goal: null
  };
}

export function draftFromThread(t: Thread): boolean {
  return t.status === DRAFT_STATUS;
}
