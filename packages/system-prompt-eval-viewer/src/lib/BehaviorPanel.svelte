<script lang="ts">
  import type { Eval } from './types';
  import BehaviorCard from './BehaviorCard.svelte';
  let { ev, eid }: { ev: Eval; eid: string } = $props();
  const defs = $derived(ev.summary.behavior_defs ?? []);
  const rates = $derived(ev.summary.per_behavior ?? {});
</script>

{#if defs.length}
  <div class="panel">
    {#each defs as d}
      <BehaviorCard def={d} rate={rates[d.id] ?? 0} cases={ev.cases} {eid} />
    {/each}
  </div>
{/if}

<style>
  .panel { display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 12px; margin: 6px 0 22px; }
</style>
