<script lang="ts">
  // The Namespace rail view: the kernel's live globals, browsable on their own
  // surface instead of interleaved into the run feed. Collects every namespace
  // `data` pane (one per Python session) and renders each as an expandable tree.
  import { onMount } from 'svelte';
  import { store } from '$lib/stream.svelte';
  import { namespaceSessions } from '$lib/namespace-sessions';
  import { buildNsItems, parseRows, nsParent, type NsRow as Row } from '$lib/namespace';
  import { setListNav } from '$lib/keys.svelte';
  import NamespaceBody from './NamespaceBody.svelte';

  // Every namespace pane, oldest session first for a stable order. A namespace pane
  // is a `data` pane with the `namespace` renderer (see introspect / pane_bridge).
  const sessions = $derived(namespaceSessions(store.panes));

  // Selection + expansion are owned here so the keyboard can walk the whole tree
  // across sessions. Each session renders with the same `s<index>` path prefix the
  // flat nav list uses, so paths line up between rendering and navigation.
  let selected = $state<string | null>(null);
  let expanded = $state<Record<string, boolean>>({});
  const prefixOf = (si: number): string => `s${si}`;

  // Parse + flatten each session once. `store.panes` is reassigned on every live
  // frame, so every body-derived value re-runs each frame; building the item list
  // here and feeding it to the header count, the nav list, and each NamespaceBody
  // keeps it to a single parse + flatten per session per frame instead of 2–3×.
  const sessionItems = $derived(
    sessions.map((s, si) => ({
      ...s,
      items: buildNsItems(parseRows(s.pane.body), expanded, prefixOf(si)),
    })),
  );

  // Total top-level names across sessions, for the header count.
  const total = $derived(
    sessionItems.reduce(
      (n, s) => n + s.items.filter((it) => it.kind === 'row' && it.depth === 0).length,
      0,
    ),
  );

  // The flattened, currently-visible rows in render order — what j/k walk.
  const flat = $derived.by(() => {
    const out: { path: string; row: Row }[] = [];
    for (const s of sessionItems) {
      for (const it of s.items) if (it.kind === 'row') out.push({ path: it.path, row: it.row });
    }
    return out;
  });

  function scrollTo(path: string | null): void {
    if (!path) return;
    queueMicrotask(() =>
      document
        .querySelector(`.nsrow-line[data-path="${CSS.escape(path)}"]`)
        ?.scrollIntoView({ block: 'nearest' }),
    );
  }
  function selectIndex(i: number): void {
    if (!flat.length) return;
    const n = Math.max(0, Math.min(flat.length - 1, i));
    selected = flat[n].path;
    scrollTo(selected);
  }
  function move(delta: number): void {
    const i = flat.findIndex((f) => f.path === selected);
    selectIndex((i < 0 ? 0 : i) + delta);
  }
  function onSelect(path: string): void {
    selected = path;
  }
  function onToggle(path: string): void {
    expanded[path] = !expanded[path];
  }
  // `l`/`o`/Enter: expand a closed container, or descend into an open one.
  function open(): void {
    const cur = flat.find((f) => f.path === selected);
    if (!cur?.row.children?.length) return;
    if (!expanded[cur.path]) {
      expanded[cur.path] = true;
    } else {
      selectIndex(flat.findIndex((f) => f.path === cur.path) + 1);
    }
  }
  // `h`: collapse an open container, else step out to the parent row.
  function back(): void {
    const cur = flat.find((f) => f.path === selected);
    if (!cur) return;
    if (cur.row.children?.length && expanded[cur.path]) {
      expanded[cur.path] = false;
      return;
    }
    const parent = nsParent(cur.path);
    if (parent) {
      selected = parent;
      scrollTo(selected);
    }
  }

  // Keep the selection valid as the namespace changes; default to the first row.
  $effect(() => {
    if (flat.length === 0) selected = null;
    else if (!flat.some((f) => f.path === selected)) selected = flat[0].path;
  });

  onMount(() => {
    setListNav({
      move,
      top: () => selectIndex(0),
      bottom: () => selectIndex(flat.length - 1),
      open,
      back,
    });
    return () => setListNav(null);
  });
</script>

<div class="nsview">
  <header class="view-head">
    <h1 class="view-title">Namespace</h1>
    {#if total > 0}<span class="view-meta">{total} {total === 1 ? 'name' : 'names'}</span>{/if}
  </header>

  <div class="nsview-body">
    {#if sessionItems.length === 0}
      <div class="view-empty">{store.live ? 'no live namespace' : 'connecting…'}</div>
    {:else}
      {#each sessionItems as s (s.key)}
        {#if sessionItems.length > 1}
          <div class="nsview-session">{s.pane.subtitle || s.pane.scope || 'session'}</div>
        {/if}
        <NamespaceBody
          pane={s.pane}
          items={s.items}
          {expanded}
          {selected}
          {onSelect}
          {onToggle}
        />
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
