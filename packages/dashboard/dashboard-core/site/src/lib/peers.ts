// Who made an edit.
//
// The document carries a `__peers` root map keyed by DECIMAL peer id, each entry
// naming what that peer is (`kind`) and what to call it (`label`). The aggregator
// registers the agents it runs; a viewer registers itself the first time it
// writes. Nothing guarantees an entry exists for a given peer -- a peer that
// edited before the table was introduced, or an agent that died before
// registering, simply is not in it -- so every read degrades to an identity built
// from the peer id itself rather than dropping the row.

export type PeerKind = 'agent' | 'human' | 'unknown';

export interface PeerInfo {
  kind: PeerKind;
  label: string;
}

// What a row renders: the label, the kind that inks it, and whether it is us.
export interface PeerBadge extends PeerInfo {
  you: boolean;
  // True when `__peers` had nothing for this peer and the label is synthesised.
  anonymous: boolean;
}

function asKind(value: unknown): PeerKind | null {
  return value === 'agent' || value === 'human' ? value : null;
}

// The last four digits of a peer id, the shortest form still distinguishing the
// handful of peers a session has. Peer ids are u64 decimal strings, so slicing
// characters is safe where `Number(...)` would round.
export function shortPeer(peerId: string): string {
  return peerId.length <= 4 ? peerId : peerId.slice(-4);
}

// Read the `__peers` projection out of `doc.toJSON()`. Anything malformed is
// skipped rather than throwing: the table is written by other processes and a
// bad entry must not take the history panel down with it.
export function readPeers(raw: unknown): Record<string, PeerInfo> {
  const out: Record<string, PeerInfo> = {};
  if (!raw || typeof raw !== 'object') return out;
  for (const [peerId, value] of Object.entries(raw as Record<string, unknown>)) {
    if (!value || typeof value !== 'object') continue;
    const entry = value as { kind?: unknown; label?: unknown };
    const kind = asKind(entry.kind);
    const label = typeof entry.label === 'string' && entry.label ? entry.label : '';
    // An entry with neither usable field is no better than an absent one.
    if (!kind && !label) continue;
    out[peerId] = { kind: kind ?? 'unknown', label: label || `peer ${shortPeer(peerId)}` };
  }
  return out;
}

// The badge for one peer. An unregistered peer still gets a stable, readable
// identity -- `you` for this browser, `peer 4821` for anyone else -- so an edit
// is never attributed to nobody.
export function peerBadge(
  peers: Record<string, PeerInfo>,
  peerId: string,
  localPeerId: string,
): PeerBadge {
  const you = peerId === localPeerId && peerId !== '';
  const known = peers[peerId];
  if (known) return { ...known, you, anonymous: false };
  return {
    kind: you ? 'human' : 'unknown',
    label: you ? 'you' : `peer ${shortPeer(peerId)}`,
    you,
    anonymous: true,
  };
}
