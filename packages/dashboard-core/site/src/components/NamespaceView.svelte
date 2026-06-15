<script lang="ts">
  // The Namespace rail view: the kernel's live globals, browsable on their own
  // surface instead of interleaved into the run feed. Collects every namespace
  // `data` pane (one per Python session) and renders each as an expandable tree.
  import { store, SCOPE_SEP } from '$lib/stream.svelte';
  import { parseRows } from '$lib/namespace';
  import type { Pane } from '$lib/types';
  import NamespaceBody from './NamespaceBody.svelte';

  // Every namespace pane, oldest scope first for a stable order. A namespace pane
  // is a `data` pane with the `namespace` renderer (see introspect / pane_bridge).
  const sessions = $derived(
    Object.keys(store.panes)
      .map((key) => {
        const sep = key.indexOf(SCOPE_SEP);
        const scope = sep === -1 ? '' : key.slice(0, sep);
        return { key, pane: { ...store.panes[key], key, scope } as Pane };
      })
      .filter((it) => (it.pane.kind ?? 'data') === 'data' && it.pane.renderer === 'namespace')
      .sort((a, b) => (a.key < b.key ? -1 : 1)),
  );

  // Total top-level names across sessions, for the header count.
  const total = $derived(sessions.reduce((n, s) => n + parseRows(s.pane.body).length, 0));
</script>

<div class="nsview">
  <header class="view-head">
    <h1 class="view-title">Namespace</h1>
    {#if total > 0}<span class="view-meta">{total} {total === 1 ? 'name' : 'names'}</span>{/if}
  </header>

  <div class="nsview-body">
    {#if sessions.length === 0}
      <div class="view-empty">{store.live ? 'no live namespace' : 'connecting…'}</div>
    {:else}
      {#each sessions as s (s.key)}
        {#if sessions.length > 1}
          <div class="nsview-session">{s.pane.subtitle || s.pane.scope || 'session'}</div>
        {/if}
        <NamespaceBody pane={s.pane} />
      {/each}
    {/if}
  </div>
</div>

<style>
  .nsview {
    flex: 1 1 auto;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg);
  }
  .nsview-body {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
    /* The tree centers in a readable measure rather than stretching edge-to-edge
       on a wide window. */
    padding: 6px clamp(8px, 2vw, 20px) 40px;
  }
  .nsview-body > :global(.ns) {
    max-width: 880px;
    margin: 0 auto;
  }
  .nsview-session {
    max-width: 880px;
    margin: 14px auto 0;
    padding: 0 14px;
    font-family: var(--mono);
    font-size: 11px;
    color: var(--ink-dim);
  }
</style>
