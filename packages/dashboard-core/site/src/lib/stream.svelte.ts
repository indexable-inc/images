// Live pane state: import the Loro doc over SSE and expose it as reactive Svelte
// state. The resources live in their owning process; the browser only reads, so
// the doc has one editor per scope and we never write back.
import { LoroDoc } from 'https://esm.sh/loro-crdt@1';
import type { PaneRecord } from './types';

export const SCOPE_SEP = String.fromCharCode(0x1f); // matches dashboard::hub::SCOPE_SEP

export const store = $state({
  panes: {} as Record<string, PaneRecord>,
  producers: 0,
  live: false,
  status: 'connecting',
});

function b64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

let started = false;
export function connect(): void {
  if (started) return;
  started = true;
  const doc = new LoroDoc();
  const es = new EventSource('/events');
  es.addEventListener('open', () => {
    store.live = true;
  });
  es.addEventListener('error', () => {
    store.live = false;
    store.status = 'reconnecting…';
  });
  const ingest = (event: MessageEvent) => {
    try {
      doc.import(b64ToBytes(event.data));
    } catch (err) {
      // A single malformed frame must not kill the listener; the next good
      // update (or a snapshot on reconnect) recovers the view.
      console.warn('dashboard: dropped malformed frame', err);
      return;
    }
    const panes = (doc.toJSON().panes ?? {}) as Record<string, PaneRecord>;
    store.panes = panes;
    const keys = Object.keys(panes);
    const scopes = new Set<string>();
    for (const k of keys) {
      const sep = k.indexOf(SCOPE_SEP);
      scopes.add(sep === -1 ? '' : k.slice(0, sep));
    }
    store.producers = scopes.size;
    const n = keys.length;
    store.status =
      `${n} pane${n === 1 ? '' : 's'}` + (scopes.size > 1 ? ` · ${scopes.size} producers` : '');
  };
  es.addEventListener('snapshot', ingest as EventListener);
  es.addEventListener('update', ingest as EventListener);
}
