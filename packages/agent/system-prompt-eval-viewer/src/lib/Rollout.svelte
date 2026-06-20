<script lang="ts">
  import type { Case } from './types';
  import Timeline from './Timeline.svelte';
  let { c, anchor }: { c: Case; anchor: string } = $props();

  function status(c: Case): { cls: string; label: string } {
    if (c.error) return { cls: 'bad', label: 'ERROR' };
    if (c.present) {
      const vals = Object.values(c.present);
      const ok = vals.filter((v) => v).length;
      const cls = ok === vals.length ? 'good' : ok ? 'warn' : 'bad';
      return { cls, label: `${ok}/${vals.length}` };
    }
    if (c.verdict) {
      const cls = c.verdict === 'validated' ? 'good' : c.verdict === 'stale' ? 'bad' : 'warn';
      return { cls, label: c.verdict };
    }
    if (c.reverse_engineered !== undefined)
      return c.reverse_engineered ? { cls: 'good', label: 'RE' } : { cls: 'bad', label: 'guessed' };
    return { cls: 'warn', label: '?' };
  }

  const st = $derived(status(c));
  const dur = $derived(c.duration_ms ? `${Math.round(c.duration_ms / 1000)}s` : '-');
  const evObj = $derived(c.present && typeof c.evidence === 'object' ? (c.evidence as Record<string, string>) : null);
</script>

<details class="rollout" id={anchor}>
  <summary>
    <span class="badge {st.cls}">{st.label}</span>
    <span class="title">{c.case_id}</span>
    <span class="roll">#{c.rollout}</span>
    <span class="meta">{dur} · {(c.output_tokens ?? 0).toLocaleString()} tok · ${(c.cost_usd ?? 0).toFixed(2)}</span>
  </summary>

  <div class="inner">
    {#if c.error}
      <p class="errline">{c.error}</p>
    {/if}

    {#if c.present}
      <div class="verdicts">
        {#each Object.entries(c.present) as [b, ok]}
          <div class="v {ok ? 'y' : 'n'}">
            <span class="mk">{ok ? '✔' : '✘'}</span>
            <span class="bn">{b}</span>
            <span class="ev">{evObj?.[b] ?? ''}</span>
          </div>
        {/each}
      </div>
    {:else if c.verdict || c.reverse_engineered !== undefined}
      <div class="answer">
        <div class="ar"><span class="k">answer</span><span>{c.answer}</span></div>
        <div class="ar"><span class="k">evidence</span><span>{typeof c.evidence === 'string' ? c.evidence : ''}</span></div>
      </div>
    {/if}

    <div class="tlh">action timeline</div>
    <Timeline steps={c.steps} transcript={c.transcript} />
  </div>
</details>

<style>
  .rollout { border: 1px solid var(--line); border-radius: 14px; margin: 8px 0; background: var(--card);
    overflow: hidden; transition: border-color 0.15s; }
  .rollout[open] { border-color: color-mix(in oklab, var(--accent) 40%, var(--line)); }
  summary { list-style: none; cursor: pointer; display: flex; align-items: center; gap: 10px;
    padding: 12px 14px; font-size: 13px; }
  summary::-webkit-details-marker { display: none; }
  .badge { min-width: 56px; text-align: center; border-radius: 999px; padding: 3px 10px; font-weight: 700;
    font-size: 12px; color: #fff; }
  .badge.good { background: var(--good); } .badge.warn { background: var(--warn); } .badge.bad { background: var(--bad); }
  .title { font-weight: 600; }
  .roll { color: var(--dim); }
  .meta { margin-left: auto; color: var(--dim); font: 12px var(--mono); }
  .inner { padding: 4px 14px 16px; }
  .errline { color: var(--bad); font: 12px/1.5 var(--mono); }
  .verdicts { display: grid; gap: 6px; margin: 6px 0 4px; }
  .v { display: grid; grid-template-columns: 18px 150px 1fr; gap: 8px; align-items: baseline;
    font-size: 13px; padding: 4px 0; border-bottom: 1px solid var(--line); }
  .v .mk { font-weight: 700; }
  .v.y .mk { color: var(--good); } .v.n .mk { color: var(--bad); }
  .v .bn { color: var(--text); } .v .ev { color: var(--dim); }
  .answer { display: grid; gap: 6px; margin: 6px 0; }
  .ar { display: grid; grid-template-columns: 90px 1fr; gap: 8px; font-size: 13px; }
  .ar .k { color: var(--dim); }
  .tlh { font-size: 11px; color: var(--dim); text-transform: uppercase; letter-spacing: 0.08em; margin: 14px 0 6px; }
</style>
