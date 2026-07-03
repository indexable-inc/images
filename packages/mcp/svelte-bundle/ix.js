// The virtual `ix` module Svelte resource components import. Resolved at
// bundle time by cli.mjs, so `import { data, act, replies } from 'ix'` works
// inside any component with no package install.
//
// It rides the wiring `Resource.script` injects into interactive resource
// HTML (`window.ix.act` / `window.ix.events`, see
// ix_notebook_mcp/runtime.py) and the initial state the kernel embeds as
// `window.__IX_STATE__`. Event shapes come from `Resource._dispatch_actions`
// and the `reply` tool:
//   {kind:'action_result', action, call, value}
//   {kind:'error',         action, call, error}
//   {kind:'reply',         text}
//
// Named `data`, not `state`: `$state` is a rune in Svelte 5, while `$data`
// stays free for the store auto-subscription syntax.
import { writable } from "svelte/store";

/** Current resource state: seeded from the kernel's embedded initial state,
 * replaced by every action handler's returned dict. Subscribe with `$data`. */
export const data = writable(globalThis.__IX_STATE__ ?? {});

/** Agent `reply` messages, newest last. Subscribe with `$replies`. */
export const replies = writable([]);

/** Last action error (string) or null. Cleared on the next successful action. */
export const error = writable(null);

/** Queue `payload` for the named in-kernel action handler. */
export function act(name, payload = {}) {
  if (!globalThis.ix?.act) {
    throw new Error(`ix.act(${JSON.stringify(name)}): this resource was registered without actions`);
  }
  return globalThis.ix.act(name, payload);
}

// A state-only component (svelte.component(..., actions=None)) gets no wiring
// script, so window.ix never exists: its `data` store just keeps the embedded
// seed and there is no event feed to subscribe to.
globalThis.ix?.events((ev) => {
  if (!ev || typeof ev !== "object") return;
  if (ev.kind === "action_result") {
    error.set(null);
    if (ev.value && typeof ev.value === "object" && !Array.isArray(ev.value)) {
      data.set(ev.value);
    }
  } else if (ev.kind === "error") {
    error.set(ev.error ?? String(ev));
  } else if (ev.kind === "reply") {
    replies.update((r) => [...r, ev.text ?? ""]);
  }
});
