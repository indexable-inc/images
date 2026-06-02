<script lang="ts">
  import { onMount } from 'svelte';
  import { store, connect } from '$lib/stream.svelte';
  import { ui, setView, startClock } from '$lib/ui.svelte';
  import { refreshRatio, bumpTheme } from '$lib/metrics.svelte';
  import { onThemeChange } from '$lib/ansi';
  import { seedDemo } from '$lib/demo';
  import Board from '$components/Board.svelte';
  import FeedView from '$components/FeedView.svelte';
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
</script>

<div class="topbar">
  <span class="mark">ix<span class="accent">·dashboard</span></span>
  <span class="dot" class:live={store.live}></span>
  <span class="viewseg">
    <button class:on={ui.view === 'feed'} onclick={() => setView('feed')}>feed</button>
    <button class:on={ui.view === 'board'} onclick={() => setView('board')}>board</button>
  </span>
  <span class="spacer"></span>
  <span class="stat">{store.status}</span>
</div>

<div class="stage">
  {#if ui.view === 'feed'}
    <FeedView />
  {:else}
    <Board />
  {/if}
  <!-- The fullscreen single-pane view overlays either surface, opened from a feed
       entry or a board card. -->
  {#if ui.focusKey}<FocusView />{/if}
</div>

<Timeline />
