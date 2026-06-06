<script lang="ts">
  // Dev-only frame rate readout. requestAnimationFrame fires at the
  // display's refresh rate (assuming the system isn't throttling), so
  // averaging deltas over a short window gives a faithful fps figure.
  // The whole component compiles out in `import.meta.env.PROD`
  // because the {#if} guard is constant-evaluated.

  let fps = $state(0);

  $effect(() => {
    if (!import.meta.env.DEV) return;
    let raf = 0;
    let frames = 0;
    let last = performance.now();
    const tick = (now: number) => {
      frames++;
      const dt = now - last;
      if (dt >= 500) {
        fps = Math.round((frames * 1000) / dt);
        frames = 0;
        last = now;
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  });
</script>

{#if import.meta.env.DEV}
  <div class="fps" title="requestAnimationFrame fps">{fps}</div>
{/if}

<style>
  .fps {
    position: fixed;
    top: 6px;
    right: 10px;
    z-index: 2000;
    padding: 1px 7px;
    border-radius: var(--radius-xs);
    background: var(--bg-pill);
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 10.5px;
    font-variant-numeric: tabular-nums;
    pointer-events: none;
    user-select: none;
  }
</style>
