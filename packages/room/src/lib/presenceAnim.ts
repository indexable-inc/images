// Shared crossfade for the viewer-stack avatars.
//
// Svelte's `crossfade` returns a (send, receive) pair that animates an
// element from its old DOM position to its new one using a shared key.
// We export one pair from this module so every ViewerStack instance
// shares the same send/receive state — when a peer's viewing_thread_id
// changes, the avatar element is "sent" from the old stack and
// "received" by the new stack under the same peer id, and Svelte
// interpolates the transform.

import { crossfade } from 'svelte/transition';
import { cubicInOut } from 'svelte/easing';

const reduceMotion =
  typeof window !== 'undefined' &&
  typeof window.matchMedia === 'function' &&
  window.matchMedia('(prefers-reduced-motion: reduce)').matches;

export const [sendViewer, receiveViewer] = crossfade({
  duration: reduceMotion ? 0 : 320,
  easing: cubicInOut,
  // Fallback when the matching counterpart hasn't mounted yet (e.g. a
  // peer that arrives without ever leaving). Just fade in/out in place
  // so nothing pops.
  fallback(node) {
    return {
      duration: reduceMotion ? 0 : 180,
      css: (t) => `opacity: ${t}; transform: scale(${0.85 + 0.15 * t})`
    };
  }
});
