// Minimal hash router.
//
// Routes:
//   #/                  -> threads list
//   #/s/<server_id>/t/<thread_id> -> thread detail
//
// We use a hash router instead of history.pushState so the SPA works
// behind the catch-all ServeDir fallback on the room server without
// any rewrite rules.

import { writable, type Readable } from 'svelte/store';
import { firstEnabledServerId } from './backend';

export type Route =
  | { name: 'threads' }
  | { name: 'thread'; serverId: string; threadId: string }
  | { name: 'not-found' };

function parse(hash: string): Route {
  const path = hash.replace(/^#/, '') || '/';
  if (path === '/' || path === '') return { name: 'threads' };
  const routedMatch = path.match(/^\/s\/([^/]+)\/t\/([^/]+)\/?$/);
  if (routedMatch) {
    return {
      name: 'thread',
      serverId: decodeURIComponent(routedMatch[1]!),
      threadId: decodeURIComponent(routedMatch[2]!)
    };
  }
  const threadMatch = path.match(/^\/t\/([^/]+)\/?$/);
  if (threadMatch) {
    const serverId = firstEnabledServerId();
    if (serverId) {
      return { name: 'thread', serverId, threadId: decodeURIComponent(threadMatch[1]!) };
    }
  }
  return { name: 'not-found' };
}

function makeRouter(): Readable<Route> & { go: (path: string) => void } {
  const store = writable<Route>(parse(window.location.hash));

  window.addEventListener('hashchange', () => {
    store.set(parse(window.location.hash));
  });

  return {
    subscribe: store.subscribe,
    go(path: string) {
      const next = path.startsWith('#') ? path : '#' + path;
      if (window.location.hash !== next) {
        window.location.hash = next;
      } else {
        // Force re-emit when the link is the same as the current route.
        store.set(parse(next));
      }
    }
  };
}

export const router = makeRouter();
