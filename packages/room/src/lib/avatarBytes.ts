// Process-wide cache for GitHub profile pictures, materialized as
// data: URLs.
//
// We keep just the github handle in Loro presence (cheap, a few
// bytes per peer), but every Avatar render that pointed at
// github.com/<handle>.png was a fresh network reference. WebKit's
// onerror fires intermittently mid-stream when presence updates
// re-render the <img>, and the fallback identicon would flash in
// for one paint before the github avatar reappeared. By fetching
// the bytes once and handing the Avatar component a stable data:
// URL, the <img src> never changes, the browser never re-requests,
// and the flash goes away.
//
// Layers:
//   - In-memory `resolved` map: lookups are O(1) and synchronous.
//   - In-memory `pending` map: dedupes concurrent fetches for the
//     same handle.
//   - localStorage: survives reloads. 24 h TTL is well under how
//     often someone changes their GitHub avatar.
//   - Negative cache: a failed fetch (404, network) is remembered
//     for the same TTL so we don't retry on every keystroke.

const STORAGE_PREFIX = 'room.avatar.v1.';
const TTL_MS = 24 * 60 * 60 * 1000;

type StoredEntry =
  | { ok: true; dataUrl: string; ts: number }
  | { ok: false; ts: number };

const resolved = new Map<string, string | null>();
const pending = new Map<string, Promise<string | null>>();

function key(handle: string, sizePx: number): string {
  return `${handle.trim().toLowerCase()}@${sizePx}`;
}

function storageKey(k: string): string {
  return STORAGE_PREFIX + k;
}

function readStored(k: string): string | null | undefined {
  if (typeof localStorage === 'undefined') return undefined;
  try {
    const raw = localStorage.getItem(storageKey(k));
    if (!raw) return undefined;
    const entry = JSON.parse(raw) as StoredEntry;
    if (Date.now() - entry.ts > TTL_MS) {
      localStorage.removeItem(storageKey(k));
      return undefined;
    }
    return entry.ok ? entry.dataUrl : null;
  } catch {
    return undefined;
  }
}

function writeStored(k: string, value: string | null): void {
  if (typeof localStorage === 'undefined') return;
  try {
    const entry: StoredEntry =
      value === null
        ? { ok: false, ts: Date.now() }
        : { ok: true, dataUrl: value, ts: Date.now() };
    localStorage.setItem(storageKey(k), JSON.stringify(entry));
  } catch {
    // quota exceeded, private mode, etc — fall back to memory only.
  }
}

function blobToDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(r.result as string);
    r.onerror = () =>
      reject(r.error ?? new Error('FileReader failed reading avatar blob'));
    r.readAsDataURL(blob);
  });
}

async function fetchOne(handle: string, sizePx: number): Promise<string | null> {
  const url = `https://github.com/${encodeURIComponent(
    handle.trim().toLowerCase()
  )}.png?size=${sizePx}`;
  try {
    const res = await fetch(url, { referrerPolicy: 'no-referrer' });
    if (!res.ok) return null;
    const blob = await res.blob();
    return await blobToDataUrl(blob);
  } catch {
    return null;
  }
}

/**
 * Synchronous lookup. Returns:
 *   - the data URL when the avatar is already resolved (memory or
 *     localStorage),
 *   - `null` when we have a negative cache entry,
 *   - `undefined` when nothing is cached yet — callers should kick
 *     off `loadGithubAvatar` and render the identicon in the meantime.
 *
 * Cheap enough to call on every Svelte effect run.
 */
export function peekGithubAvatar(
  handle: string | null | undefined,
  size: number
): string | null | undefined {
  if (!handle) return null;
  const sizePx = Math.ceil(size * 2);
  const k = key(handle, sizePx);
  if (resolved.has(k)) return resolved.get(k)!;
  const stored = readStored(k);
  if (stored !== undefined) {
    resolved.set(k, stored);
    return stored;
  }
  return undefined;
}

/**
 * Resolve the avatar URL, fetching once if needed. Dedupes
 * concurrent requests for the same (handle, size). Always commits
 * the result (success or failure) to the in-memory and localStorage
 * caches so subsequent `peek()` calls are synchronous.
 */
export function loadGithubAvatar(
  handle: string,
  size: number
): Promise<string | null> {
  const trimmed = handle.trim().toLowerCase();
  if (!trimmed) return Promise.resolve(null);
  const sizePx = Math.ceil(size * 2);
  const k = key(trimmed, sizePx);

  if (resolved.has(k)) return Promise.resolve(resolved.get(k)!);
  const stored = readStored(k);
  if (stored !== undefined) {
    resolved.set(k, stored);
    return Promise.resolve(stored);
  }
  const existing = pending.get(k);
  if (existing) return existing;

  const p = fetchOne(trimmed, sizePx).then((value) => {
    resolved.set(k, value);
    writeStored(k, value);
    pending.delete(k);
    return value;
  });
  pending.set(k, p);
  return p;
}
