<script lang="ts">
  // Big current value + tiny history bars. DOM/CSS only, still by design.
  let { points, label, unit = "", warnAt = Infinity }: {
    points: number[]; label: string; unit?: string; warnAt?: number;
  } = $props();
  const now = $derived(points.at(-1) ?? 0);
  const max = $derived(Math.max(1, ...points));
</script>

<div class="gauge" class:warn={now >= warnAt}>
  <div class="value">{now}<span class="unit">{unit}</span></div>
  <div class="bars">
    {#each points.slice(-36) as p}
      <div class="bar" style="height: {Math.max(6, (p / max) * 100)}%"></div>
    {/each}
  </div>
  <div class="label">{label}</div>
</div>

<style>
  .gauge { flex: 1; min-width: 7.5rem; }
  .value { font-size: 1.6rem; font-weight: 650; font-variant-numeric: tabular-nums; line-height: 1.1; }
  .warn .value { color: #e05252; }
  .unit { font-size: 0.85rem; color: var(--muted); margin-left: 0.1rem; }
  .bars { display: flex; align-items: flex-end; gap: 2px; height: 1.6rem; margin-top: 0.35rem; }
  .bar { flex: 1; background: var(--border); min-height: 2px; }
  .bar:last-child { background: var(--muted); }
  .warn .bar:last-child { background: #e05252; }
  .label { font-size: 0.72rem; color: var(--muted); text-transform: uppercase; letter-spacing: 0.06em; margin-top: 0.3rem; }
</style>
