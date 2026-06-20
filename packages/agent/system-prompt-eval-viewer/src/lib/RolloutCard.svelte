<script lang="ts">
  import type { Case } from './types';
  import { statusOf } from './types';
  import Badge from './ui/Badge.svelte';
  import VerdictTable from './VerdictTable.svelte';
  import Timeline from './Timeline.svelte';

  let { c, anchor }: { c: Case; anchor: string } = $props();
  let open = $state(false);
  const st = $derived(statusOf(c));
  const dur = $derived(c.duration_ms ? `${Math.round(c.duration_ms / 1000)}s` : '-');
</script>

<!-- Only mount the (potentially many-MB) verdicts + timeline once the card is
     opened, so a report with dozens of rollouts paints instantly. bind:open also
     tracks the toolbar's expand-all (which toggles the native details). -->
<details class="rollout" id={anchor} bind:open>
  <summary>
    <Badge kind={st.kind} text={st.label} />
    <span class="title">{c.case_id}</span>
    <span class="roll">#{c.rollout}</span>
    <span class="meta">{dur} · {(c.output_tokens ?? 0).toLocaleString()} tok · ${(c.cost_usd ?? 0).toFixed(2)}</span>
  </summary>
  {#if open}
    <div class="inner">
      {#if c.error}<p class="errline">{c.error}</p>{/if}
      <VerdictTable {c} />
      <div class="tlh">action timeline</div>
      <Timeline steps={c.steps} transcript={c.transcript} />
    </div>
  {/if}
</details>

<style>
  .rollout { border: 1px solid var(--line); border-radius: var(--radius); margin: 6px 0; background: var(--card); overflow: hidden; }
  .rollout[open] { background: var(--panel); }
  summary { list-style: none; cursor: pointer; display: flex; align-items: center; gap: 10px; padding: 11px 14px; font-size: 13px; }
  summary:hover { background: var(--panel); }
  summary::-webkit-details-marker { display: none; }
  .title { font-weight: 600; }
  .roll { color: var(--dim); }
  .meta { margin-left: auto; color: var(--dim); font: 12px var(--mono); }
  .inner { padding: 4px 14px 16px; }
  .errline { color: var(--bad); font: 12px/1.5 var(--mono); }
  .tlh { font-size: 11px; color: var(--dim); text-transform: uppercase; letter-spacing: 0.06em; margin: 14px 0 6px; }
</style>
