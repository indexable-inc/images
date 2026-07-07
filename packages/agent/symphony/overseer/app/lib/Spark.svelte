<script lang="ts">
  // CSS bar sparkline: one div per point, height normalized to the series
  // max. Still by design; the page reloads itself for fresh data.
  let { points, label, accent = false }: { points: number[]; label: string; accent?: boolean } = $props();
  const max = $derived(Math.max(1, ...points));
</script>

<div class="spark">
  <div class="bars" class:accent>
    {#each points as p}
      <div class="bar" style="height: {Math.max(4, (p / max) * 100)}%" title={String(p)}></div>
    {/each}
  </div>
  <div class="caption">
    <span>{label}</span>
    <span class="now">{points.at(-1) ?? 0}</span>
  </div>
</div>

<style>
  .spark { flex: 1; min-width: 10rem; }
  .bars { display: flex; align-items: flex-end; gap: 2px; height: 2.4rem; }
  .bar { flex: 1; background: var(--border); min-height: 2px; }
  .bars.accent .bar:last-child { background: #3fb96f; }
  .bars:not(.accent) .bar:last-child { background: var(--muted); }
  .caption { display: flex; justify-content: space-between; font-size: 0.75rem; color: var(--muted); margin-top: 0.3rem; }
  .now { font-variant-numeric: tabular-nums; color: var(--fg); }
</style>
