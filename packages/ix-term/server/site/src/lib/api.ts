import type { SessionMeta } from '$lib/types';

export async function listSessions(): Promise<SessionMeta[]> {
  const res = await fetch('/api/sessions');
  if (!res.ok) {
    throw new Error(`listing sessions failed: ${String(res.status)}`);
  }
  return (await res.json()) as SessionMeta[];
}

export async function createSession(name?: string): Promise<SessionMeta> {
  const res = await fetch('/api/sessions', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(name === undefined ? {} : { name })
  });
  if (!res.ok) {
    throw new Error(`creating session failed: ${String(res.status)}`);
  }
  return (await res.json()) as SessionMeta;
}

export async function renameSession(id: string, name: string): Promise<void> {
  const res = await fetch(`/api/sessions/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ name })
  });
  if (!res.ok) {
    throw new Error(`renaming session failed: ${String(res.status)}`);
  }
}

export async function deleteSession(id: string): Promise<void> {
  const res = await fetch(`/api/sessions/${encodeURIComponent(id)}`, {
    method: 'DELETE'
  });
  if (!res.ok) {
    throw new Error(`deleting session failed: ${String(res.status)}`);
  }
}
