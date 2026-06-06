// Local browser identity. Each browser gets a stable random handle so
// presence/chat can attribute activity without an auth system. The
// room is shared with EVERYONE — this is just a display name.
//
// Two identity kinds:
//   - `anon`   : display name + deterministic identicon SVG (default).
//   - `github` : display name = github handle; avatar fetched from
//                github.com/<handle>.png so peers see your real
//                profile picture.

const STORAGE_KEY = 'room.identity.v2';
const LEGACY_KEY = 'room.identity.v1';

export type IdentityKind = 'anon' | 'github';

export interface Identity {
  id: string;
  name: string;
  kind: IdentityKind;
  /** GitHub handle when `kind === 'github'`. Lowercased on save. */
  github?: string;
}

const ANIMALS = [
  'otter', 'fox', 'heron', 'lynx', 'panda', 'whale', 'koala', 'crane',
  'badger', 'puffin', 'wolf', 'seal', 'finch', 'ibex', 'tapir', 'gecko'
];

function randomName(): string {
  const animal = ANIMALS[Math.floor(Math.random() * ANIMALS.length)]!;
  const n = Math.floor(Math.random() * 900) + 100;
  return `${animal}-${n}`;
}

function randomId(): string {
  const arr = new Uint8Array(8);
  crypto.getRandomValues(arr);
  return [...arr].map((b) => b.toString(16).padStart(2, '0')).join('');
}

// GitHub username constraints: 1–39 chars, alphanumeric + dashes, no
// leading/trailing dash. Anything else is treated as a free-form
// display name and falls back to the anon identicon.
const GITHUB_RE = /^[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,37}[a-zA-Z0-9])?$/;

export function isValidGithubHandle(s: string): boolean {
  return GITHUB_RE.test(s.trim());
}

// Per-window ephemeral identity override. The Rust `spawn_window`
// command opens new windows with `?as=<name>` so each window appears
// as a distinct user in the presence stack — handy for testing the
// multiplayer flow without juggling browsers. The override is held in
// memory (not written to localStorage) so closing the window doesn't
// pollute the main identity.
let cachedOverride: Identity | null | undefined;

function readOverride(): Identity | null {
  if (cachedOverride !== undefined) return cachedOverride;
  if (typeof window === 'undefined') return (cachedOverride = null);
  try {
    const params = new URLSearchParams(window.location.search);
    const as = params.get('as');
    if (!as) return (cachedOverride = null);
    cachedOverride = { id: 'as-' + as, name: as, kind: 'anon' };
    return cachedOverride;
  } catch {
    return (cachedOverride = null);
  }
}

function readStored(): Identity | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<Identity>;
      if (parsed.id && parsed.name) {
        return normalize(parsed as Identity);
      }
    }
    // One-shot migration from the v1 schema (no kind, no github).
    const legacy = localStorage.getItem(LEGACY_KEY);
    if (legacy) {
      const parsed = JSON.parse(legacy) as { id?: string; name?: string };
      if (parsed.id && parsed.name) {
        const migrated: Identity = {
          id: parsed.id,
          name: parsed.name,
          kind: 'anon'
        };
        localStorage.setItem(STORAGE_KEY, JSON.stringify(migrated));
        return migrated;
      }
    }
  } catch {
    // ignore
  }
  return null;
}

function normalize(id: Identity): Identity {
  const kind: IdentityKind = id.kind === 'github' ? 'github' : 'anon';
  const out: Identity = { id: id.id, name: id.name, kind };
  if (kind === 'github' && id.github) out.github = id.github.toLowerCase();
  return out;
}

export function loadIdentity(): Identity {
  const override = readOverride();
  if (override) return override;
  const stored = readStored();
  if (stored) return stored;
  const fresh: Identity = { id: randomId(), name: randomName(), kind: 'anon' };
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(fresh));
  } catch {
    // private mode etc
  }
  return fresh;
}

/** Replace the current identity. Renames in a ?as= window update the
 *  in-memory override only; otherwise we persist. */
export function setIdentity(next: Partial<Identity>): Identity {
  const current = loadIdentity();
  const merged = normalize({
    id: current.id,
    name: (next.name ?? current.name).trim().slice(0, 39) || randomName(),
    kind: next.kind ?? current.kind,
    github: next.github ?? current.github
  });
  // If kind is anon, the github handle is irrelevant — drop it so it
  // doesn't ship through presence and confuse the receiver.
  if (merged.kind === 'anon') delete merged.github;

  const override = readOverride();
  if (override) {
    cachedOverride = merged;
    return merged;
  }
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(merged));
  } catch {
    // ignore
  }
  return merged;
}

/** Back-compat helper kept for callers that only want to change the
 *  display name without touching kind/github. */
export function setIdentityName(name: string): Identity {
  return setIdentity({ name });
}
