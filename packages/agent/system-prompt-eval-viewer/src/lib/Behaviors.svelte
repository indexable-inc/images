<script lang="ts">
  import type { Eval } from './types';
  import { pct, grade } from './types';
  let { ev, eid }: { ev: Eval; eid: string } = $props();

  const defs = $derived(ev.summary.behavior_defs ?? []);
  const rates = $derived(ev.summary.per_behavior ?? {});
</script>

{#if defs.length}
  <div class="panel">
    {#each defs as d}
      {@const rate = rates[d.id] ?? 0}
      <div class="beh">
        <div class="bh">
          <b>{d.name}</b>
          <span class="rate {grade(rate)}">{pct(rate)}</span>
        </div>
        <div class="bar"><span class="{grade(rate)}" style="width:{rate * 100}%"></span></div>
        <p class="rubric">{d.rubric}</p>
        <div class="dots">
          {#each ev.cases as c, i}
            {#if c.present && d.id in c.present}
              <a class="dot {c.present[d.id] ? 'y' : 'n'}" href="#{eid}-{i}" title="{c.case_id} #{c.rollout}">
                {c.present[d.id] ? '✔' : '✘'}
              </a>
            {/if}
          {/each}
        </div>
      </div>
    {/each}
  </div>
{/if}

<style>
  .panel { display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 14px; margin: 6px 0 18px; }
  .beh { border: 1px solid var(--line); border-radius: 14px; padding: 14px 16px; background: var(--card); }
  .bh { display: flex; justify-content: space-between; align-items: baseline; }
  .bh b { font-size: 14px; }
  .rate { font-weight: 800; font-size: 15px; }
  .rate.good { color: var(--good); } .rate.warn { color: var(--warn); } .rate.bad { color: var(--bad); }
  .bar { height: 8px; border-radius: 999px; background: var(--chip); overflow: hidden; margin: 8px 0; }
  .bar span { display: block; height: 100%; border-radius: 999px; }
  .bar span.good { background: linear-gradient(90deg, var(--good), color-mix(in oklab, var(--good) 60%, var(--accent))); }
  .bar span.warn { background: var(--warn); }
  .bar span.bad { background: var(--bad); }
  .rubric { color: var(--dim); font-size: 12.5px; line-height: 1.5; margin: 4px 0 10px; }
  .dots { display: flex; flex-wrap: wrap; gap: 4px; }
  .dot { width: 22px; height: 22px; border-radius: 7px; display: grid; place-items: center; font-size: 11px;
    color: #fff; text-decoration: none; transition: transform 0.1s; }
  .dot:hover { transform: translateY(-1px); }
  .dot.y { background: var(--good); } .dot.n { background: var(--bad); }
</style>
