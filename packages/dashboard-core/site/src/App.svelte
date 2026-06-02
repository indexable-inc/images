<script lang="ts">
  import { onMount } from 'svelte';
  import { store, connect } from '$lib/stream.svelte';
  import { ui, startClock } from '$lib/ui.svelte';
  import { refreshRatio, bumpTheme } from '$lib/metrics.svelte';
  import { onThemeChange } from '$lib/ansi';
  import Board from '$components/Board.svelte';
  import FocusView from '$components/FocusView.svelte';
  import Timeline from '$components/Timeline.svelte';

  onMount(() => {
    refreshRatio();
    startClock();
    connect();
    // Remeasure once Berkeley Mono (if present) loads, and repaint on a theme
    // flip so chrome and terminal palette stay in sync.
    if (document.fonts?.ready) document.fonts.ready.then(refreshRatio);
    return onThemeChange(bumpTheme);
  });
</script>

<div class="topbar">
  <span class="mark">ix<span class="accent">·dashboard</span></span>
  <span class="dot" class:live={store.live}></span>
  <span class="spacer"></span>
  <span class="stat">{store.status}</span>
</div>

<div class="stage">
  <Board />
  {#if ui.focusKey}<FocusView />{/if}
</div>

<Timeline />
