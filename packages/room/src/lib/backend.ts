// Room server registry.
//
// The UI can connect to several independent room-server processes at
// once. Each server owns its own SQLite store, WebTransport session,
// and Loro room doc; the client merges those facts into one view.

import { derived, writable, get, type Readable } from 'svelte/store';

const SERVERS_KEY = 'room.servers.v1';
const LEGACY_BACKEND_KEY = 'room.backend.url';
const TAURI_LOCAL_FALLBACK = 'http://localhost:8080';

const baked = import.meta.env.VITE_ROOM_BACKEND_URL as string | undefined;

export interface RoomServer {
  id: string;
  name: string;
  httpBase: string;
  enabled: boolean;
  managed?: boolean;
  /** Codex runtime backing a managed backend ("host" or "ixvm"). */
  runtime?: string | null;
}

function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

function normalize(url: string): string {
  return url.trim().replace(/\/+$/, '');
}

function validateHttpBase(url: string): string {
  const norm = normalize(url);
  if (norm === '') return '';
  const parsed = new URL(norm, window.location.href);
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    throw new Error('server URL must use http or https');
  }
  if (!parsed.host) throw new Error('server URL must include a host');
  return norm;
}

function slug(input: string): string {
  return input
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 48);
}

function defaultBase(): string {
  if (isTauriRuntime() && baked) return normalize(baked);
  if (isTauriRuntime()) return TAURI_LOCAL_FALLBACK;
  return '';
}

function defaultServer(): RoomServer {
  const base = defaultBase();
  return {
    id: 'default',
    name: base ? new URL(base, window.location.href).host : 'Current origin',
    httpBase: base,
    enabled: true
  };
}

function migrateLegacy(): RoomServer[] | null {
  if (typeof localStorage === 'undefined') return null;
  const legacy = localStorage.getItem(LEGACY_BACKEND_KEY);
  if (!legacy) return null;
  const base = validateHttpBase(legacy);
  return [
    {
      id: 'default',
      name: new URL(base, window.location.href).host,
      httpBase: base,
      enabled: true
    }
  ];
}

function loadServers(): RoomServer[] {
  if (typeof localStorage === 'undefined') return [defaultServer()];
  try {
    const raw = localStorage.getItem(SERVERS_KEY);
    if (!raw) {
      return migrateLegacy() ?? [defaultServer()];
    }
    const parsed = JSON.parse(raw) as RoomServer[];
    const servers = parsed
      .filter((s) => s && typeof s.id === 'string' && typeof s.httpBase === 'string')
      .map((s) => ({
        id: slug(s.id) || crypto.randomUUID(),
        name: String(s.name || s.httpBase || 'Server'),
        httpBase: normalize(s.httpBase),
        enabled: s.enabled !== false
      }));
    return servers.length > 0 ? servers : [defaultServer()];
  } catch {
    return [defaultServer()];
  }
}

function saveServers(next: RoomServer[]): void {
  if (typeof localStorage === 'undefined') return;
  localStorage.setItem(SERVERS_KEY, JSON.stringify(next));
}

const serversStore = writable<RoomServer[]>(loadServers());
const backendServersStore = writable<RoomServer[]>([]);

function commit(next: RoomServer[]): void {
  serversStore.set(next);
  saveServers(next);
}

export const roomServers: Readable<RoomServer[]> = derived(
  [serversStore, backendServersStore],
  ([$servers, $backends]) => {
    const userIds = new Set($servers.map((s) => s.id));
    return [...$servers, ...$backends.filter((s) => !userIds.has(s.id))];
  }
);

export function listRoomServers(): RoomServer[] {
  return get(serversStore);
}

export function enabledRoomServers(): RoomServer[] {
  return get(serversStore).filter((s) => s.enabled);
}

// Resolve across BOTH user-configured servers and auto-registered managed
// backends (the `backend-*` symphony run servers): callers like
// `backendHttpBase` and the App route guard must see managed backends too, or
// every request to a symphony run throws "unknown room server" and its threads
// never load (empty sidebar). User-server management writes `serversStore`
// directly, so broadening this read does not affect add/remove/enable.
export function getRoomServer(serverId: string): RoomServer | undefined {
  return get(roomServers).find((s) => s.id === serverId);
}

export function firstEnabledServerId(): string | null {
  return enabledRoomServers()[0]?.id ?? null;
}

export function backendHttpBase(serverId: string): string {
  const server = getRoomServer(serverId);
  if (!server) throw new Error('unknown room server: ' + serverId);
  return server.httpBase;
}

export interface UpsertServerInput {
  id?: string;
  name: string;
  httpBase: string;
  enabled?: boolean;
}

export function upsertRoomServer(input: UpsertServerInput): RoomServer {
  const httpBase = validateHttpBase(input.httpBase);
  const name = input.name.trim() || new URL(httpBase, window.location.href).host;
  const current = get(serversStore);
  const idBase = input.id ?? slug(name) ?? 'server';
  let id = slug(idBase) || crypto.randomUUID();
  if (!input.id) {
    const taken = new Set(current.map((s) => s.id));
    const root = id;
    let n = 2;
    while (taken.has(id)) id = `${root}-${n++}`;
  }
  const nextServer: RoomServer = {
    id,
    name,
    httpBase,
    enabled: input.enabled ?? true
  };
  const next = current.some((s) => s.id === id)
    ? current.map((s) => (s.id === id ? nextServer : s))
    : [...current, nextServer];
  commit(next);
  return nextServer;
}

export function removeRoomServer(serverId: string): void {
  const next = get(serversStore).filter((s) => s.id !== serverId);
  commit(next.length > 0 ? next : [defaultServer()]);
}

export function setRoomServerEnabled(serverId: string, enabled: boolean): void {
  commit(get(serversStore).map((s) => (s.id === serverId ? { ...s, enabled } : s)));
}

interface RegisteredBackend {
  id: string;
  name: string;
  url: string;
  source: string;
  status: string;
  runtime?: string | null;
}

async function refreshRegisteredBackends(): Promise<void> {
  if (isTauriRuntime()) return;
  const resp = await fetch('/api/backends');
  if (!resp.ok) throw new Error('/api/backends -> ' + resp.status);
  const body = (await resp.json()) as { backends?: RegisteredBackend[] };
  const next = (body.backends ?? [])
    .filter((backend) => backend.status === 'active')
    .map((backend) => ({
      id: 'backend-' + backend.id,
      name: backend.name || backend.id,
      httpBase: '/api/backends/' + encodeURIComponent(backend.id) + '/proxy',
      enabled: true,
      managed: true,
      runtime: backend.runtime ?? null
    }));
  backendServersStore.set(next);
}

if (typeof window !== 'undefined') {
  void refreshRegisteredBackends().catch((err) => {
    console.warn('room: backend registry refresh failed', err);
  });
  window.setInterval(() => {
    void refreshRegisteredBackends().catch((err) => {
      console.warn('room: backend registry refresh failed', err);
    });
  }, 5000);
}

export interface WtInfo {
  /** `https://host:port` for `new WebTransport(...)`. */
  wtUrl: string;
  /** SHA-256 of the server cert as a `Uint8Array` ready to drop into
   *  the `serverCertificateHashes` constructor option. */
  certHash: Uint8Array;
}

export async function fetchWtInfo(serverId: string, signal?: AbortSignal): Promise<WtInfo> {
  const base = backendHttpBase(serverId);
  const resp = await fetch(`${base}/api/wt/info`, { signal });
  if (!resp.ok) {
    throw new Error(`GET /api/wt/info -> ${resp.status}`);
  }
  const body = (await resp.json()) as { wt_url: string; cert_sha256_hex: string };
  if (!body.wt_url || !body.cert_sha256_hex) {
    throw new Error('malformed /api/wt/info response');
  }
  return { wtUrl: body.wt_url, certHash: hexToBytes(body.cert_sha256_hex) };
}

function hexToBytes(hex: string): Uint8Array {
  if (hex.length % 2 !== 0) throw new Error('cert hash hex has odd length');
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}
