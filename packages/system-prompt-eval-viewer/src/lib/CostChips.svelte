<script lang="ts">
  import type { Summary } from './types';
  import Chip from './ui/Chip.svelte';
  let { summary }: { summary: Summary } = $props();
  const c = $derived(summary.cost);
</script>

{#if c}
  <div class="row">
    {#if c.mean_duration_s != null}<Chip label="mean" value={`${Math.round(c.mean_duration_s)}s`} />{/if}
    {#if c.total_output_tokens != null}<Chip label="out" value={`${c.total_output_tokens.toLocaleString()} tok`} />{/if}
    {#if c.total_cost_usd != null}<Chip label="cost" value={`$${c.total_cost_usd.toFixed(2)}`} />{/if}
    {#if summary.sandbox != null}<Chip label="sandbox" value={summary.sandbox} />{/if}
  </div>
{/if}

<style>
  .row { display: flex; flex-wrap: wrap; gap: 6px; margin: 8px 0 14px; }
</style>
