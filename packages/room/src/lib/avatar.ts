// Deterministic SVG avatars generated from the display-name hash.
//
// Uses DiceBear's `identicon` style — the same kind of 5x5 mirrored
// pixel grid GitHub serves as default profile pictures. The
// foreground color is derived from a per-name hash so two adjacent
// users almost never share a color, mirroring GitHub's wide-hue
// identicon palette. Generation is synchronous and memoized; the
// result is a small SVG string we drop in via {@html}.

import { createAvatar } from '@dicebear/core';
import { identicon } from '@dicebear/collection';

const cache = new Map<string, string>();

// FNV-1a accumulation + Murmur3 finalizer. The finalizer's three
// xorshift-and-multiply rounds avalanche the bits so similar strings
// (or strings with shared affixes) end up far apart in the output.
// Plain FNV-1a alone clustered the per-name hues into a narrow band.
function hash32(input: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < input.length; i++) {
    h ^= input.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  h ^= h >>> 16;
  h = Math.imul(h, 0x85ebca6b);
  h ^= h >>> 13;
  h = Math.imul(h, 0xc2b2ae35);
  h ^= h >>> 16;
  return h >>> 0;
}

function hueFor(seed: string): number {
  return Math.floor((hash32(seed) / 0x100000000) * 360);
}

function hslToHex(h: number, s: number, l: number): string {
  const sN = s / 100;
  const lN = l / 100;
  const k = (n: number) => (n + h / 30) % 12;
  const a = sN * Math.min(lN, 1 - lN);
  const f = (n: number) => {
    const v = lN - a * Math.max(-1, Math.min(k(n) - 3, Math.min(9 - k(n), 1)));
    return Math.round(v * 255).toString(16).padStart(2, '0');
  };
  return `${f(0)}${f(8)}${f(4)}`;
}

// The avatar's foreground color, exposed so other UI (peer cursors,
// presence dots) can match what the user already associates with
// each peer. Keep in lockstep with the `rowColor` line in avatarSvg
// below — same hue + saturation + lightness.
export function avatarColor(seed: string): string {
  const key = seed || 'anon';
  return `hsl(${hueFor(key)} 62% 48%)`;
}

export function avatarSvg(seed: string): string {
  const key = seed || 'anon';
  const hit = cache.get(key);
  if (hit) return hit;
  const hue = hueFor(key);
  // Saturation/lightness chosen to read well on both light and dark
  // sidebar backgrounds without going neon.
  const fg = hslToHex(hue, 62, 48);
  const bg = hslToHex(hue, 30, 92);
  const svg = createAvatar(identicon, {
    seed: key,
    backgroundColor: [bg],
    backgroundType: ['solid'],
    rowColor: [fg]
  }).toString();
  cache.set(key, svg);
  return svg;
}

export function avatarDataUrl(seed: string): string {
  return `data:image/svg+xml;utf8,${encodeURIComponent(avatarSvg(seed))}`;
}

/** GitHub's canonical avatar URL for a username. `size` is the
 *  rendered side length in CSS pixels — we ask GitHub for a 2x
 *  variant so retina displays stay crisp. */
export function githubAvatarUrl(username: string, size: number): string {
  const u = username.trim().toLowerCase();
  return `https://github.com/${encodeURIComponent(u)}.png?size=${Math.ceil(size * 2)}`;
}
