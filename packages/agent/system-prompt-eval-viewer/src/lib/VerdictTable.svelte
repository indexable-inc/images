<script lang="ts">
  import type { Case } from './types';
  let { c }: { c: Case } = $props();
  const evObj = $derived(c.present && typeof c.evidence === 'object' ? (c.evidence as Record<string, string>) : null);
</script>

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
    {#if c.verdict}<div class="ar"><span class="k">verdict</span><span>{c.verdict}</span></div>{/if}
    <div class="ar"><span class="k">answer</span><span>{c.answer}</span></div>
    <div class="ar"><span class="k">evidence</span><span>{typeof c.evidence === 'string' ? c.evidence : ''}</span></div>
  </div>
{/if}

<style>
  .verdicts { display: grid; gap: 2px; margin: 4px 0; }
  .v { display: grid; grid-template-columns: 18px 150px 1fr; gap: 8px; align-items: baseline;
    font-size: 13px; padding: 5px 0; border-bottom: 1px solid var(--line); }
  .v .mk { font-weight: 700; }
  .v.y .mk { color: var(--good); }
  .v.n .mk { color: var(--bad); }
  .v .ev { color: var(--dim); }
  .answer { display: grid; gap: 6px; margin: 4px 0; }
  .ar { display: grid; grid-template-columns: 90px 1fr; gap: 8px; font-size: 13px; }
  .ar .k { color: var(--dim); }
</style>
