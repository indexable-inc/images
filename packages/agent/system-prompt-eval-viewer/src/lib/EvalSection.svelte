<script lang="ts">
  import type { Eval } from './types';
  import Score from './ui/Score.svelte';
  import CostChips from './CostChips.svelte';
  import BehaviorPanel from './BehaviorPanel.svelte';
  import RolloutCard from './RolloutCard.svelte';

  let { name, ev, idx }: { name: string; ev: Eval; idx: number } = $props();
  const eid = $derived(`e${idx}`);
</script>

<section>
  <div class="sh" id={name}>
    <h2>{name}</h2>
    <Score value={ev.headline} />
  </div>
  <CostChips summary={ev.summary} />
  <BehaviorPanel {ev} {eid} />
  <h3>rollouts</h3>
  {#each ev.cases as c, j}
    <RolloutCard {c} anchor={`${eid}-${j}`} />
  {/each}
</section>

<style>
  section { margin-top: 26px; }
  .sh { display: flex; align-items: baseline; justify-content: space-between; border-top: 1px solid var(--line); padding-top: 18px; }
  h2 { font-size: 17px; font-weight: 600; margin: 0; }
  h3 { font-size: 11px; color: var(--dim); text-transform: uppercase; letter-spacing: 0.06em; margin: 16px 0 6px; }
</style>
