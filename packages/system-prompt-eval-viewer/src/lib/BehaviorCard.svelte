<script lang="ts">
  import type { BehaviorDef, Case } from './types';
  import Score from './ui/Score.svelte';
  import Bar from './ui/Bar.svelte';
  import Dot from './ui/Dot.svelte';

  let { def, rate, cases, eid }: { def: BehaviorDef; rate: number; cases: Case[]; eid: string } = $props();
</script>

<div class="beh">
  <div class="bh">
    <b>{def.name}</b>
    <Score value={rate} size="sm" />
  </div>
  <Bar value={rate} />
  <p class="rubric">{def.rubric}</p>
  <div class="dots">
    {#each cases as c, i}
      {#if c.present && def.id in c.present}
        <Dot ok={c.present[def.id]} href={`#${eid}-${i}`} title={`${c.case_id} #${c.rollout}`} />
      {/if}
    {/each}
  </div>
</div>

<style>
  .beh { border: 1px solid var(--line); border-radius: var(--radius); padding: 14px 16px; background: var(--card); }
  .bh { display: flex; justify-content: space-between; align-items: baseline; }
  .bh b { font-size: 14px; font-weight: 600; }
  .rubric { color: var(--dim); font-size: 12.5px; line-height: 1.5; margin: 4px 0 10px; }
  .dots { display: flex; flex-wrap: wrap; gap: 4px; }
</style>
