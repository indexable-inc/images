<script lang="ts">
  import { onMount } from 'svelte';
  import { store, connect } from '$lib/stream.svelte';
  import { ui, setView, startClock } from '$lib/ui.svelte';
  import { refreshRatio, bumpTheme } from '$lib/metrics.svelte';
  import { onThemeChange } from '$lib/ansi';
  import { seedDemo } from '$lib/demo';
  import Board from '$components/Board.svelte';
  import FeedView from '$components/FeedView.svelte';
  import NamespaceView from '$components/NamespaceView.svelte';
  import FocusView from '$components/FocusView.svelte';
  import Timeline from '$components/Timeline.svelte';

  onMount(() => {
    refreshRatio();
    startClock();
    // `?demo` seeds front-end-only sample panes (no hub needed) to explore the UI;
    // otherwise connect to the live aggregator.
    if (new URLSearchParams(location.search).has('demo')) {
      seedDemo();
    } else {
      connect();
    }
    // Remeasure once Berkeley Mono (if present) loads, and repaint on a theme
    // flip so chrome and terminal palette stay in sync.
    if (document.fonts?.ready) document.fonts.ready.then(refreshRatio);
    return onThemeChange(bumpTheme);
  });

  // The icon rail's tabs, in order. Each drives the top-level view; the SVG is a
  // minimal 1.7px-stroke glyph so the rail stays quiet.
  const tabs = [
    { view: 'feed' as const, label: 'Jobs' },
    { view: 'namespace' as const, label: 'Namespace' },
    { view: 'board' as const, label: 'Board' },
  ];
</script>

<!-- The Obsidian shell: a thin icon rail on the left drives the view; the
     workspace (the active surface + the shared timeline) fills the rest. -->
<nav class="rail">
  <div class="rail-logo">ix</div>
  {#each tabs as t (t.view)}
    <button
      class="rail-btn"
      class:on={ui.view === t.view}
      title={t.label}
      aria-label={t.label}
      aria-current={ui.view === t.view}
      onclick={() => setView(t.view)}
    >
      {#if t.view === 'feed'}
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7"><rect x="3" y="4" width="18" height="4" rx="1" /><rect x="3" y="11" width="18" height="4" rx="1" /><rect x="3" y="18" width="18" height="3" rx="1" /></svg>
      {:else if t.view === 'namespace'}
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7"><path d="M12 3 3 7.5v9L12 21l9-4.5v-9L12 3zM3 7.5 12 12l9-4.5M12 12v9" /></svg>
      {:else}
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7"><rect x="3" y="3" width="7" height="7" rx="1" /><rect x="14" y="3" width="7" height="7" rx="1" /><rect x="3" y="14" width="7" height="7" rx="1" /><rect x="14" y="14" width="7" height="7" rx="1" /></svg>
      {/if}
    </button>
  {/each}
  <div class="rail-spacer"></div>
  <div class="rail-health" title={store.live ? 'connected' : store.status}>
    <span class="rail-pulse" class:live={store.live}></span>
  </div>
</nav>

<div class="workspace">
  <div class="stage">
    {#if ui.view === 'feed'}
      <FeedView />
    {:else if ui.view === 'namespace'}
      <NamespaceView />
    {:else}
      <Board />
    {/if}
    <!-- The fullscreen single-pane view overlays the active surface, opened from a
         feed entry or a board card. -->
    {#if ui.focusKey}<FocusView />{/if}
  </div>

  <Timeline />
</div>
