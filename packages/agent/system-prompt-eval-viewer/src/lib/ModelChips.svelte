<script lang="ts">
  import type { Summary } from './types';
  let { summary }: { summary: Summary } = $props();
</script>

{#if summary.agent_model || summary.judge_model}
  <div class="models">
    {#if summary.agent_model}
      <span class="m agent" title="model under test">
        <span class="k">agent</span>{summary.agent_model}{#if summary.effort}<span class="eff">· {summary.effort}</span>{/if}
      </span>
    {/if}
    {#if summary.judge_model}
      <span class="m judge" title="LLM judge model"><span class="k">judge</span>{summary.judge_model}</span>
    {/if}
  </div>
{/if}

<style>
  .models { display: flex; flex-wrap: wrap; gap: 8px; margin: 4px 0 12px; }
  .m { display: inline-flex; align-items: baseline; gap: 6px; font-size: 12.5px; font-weight: 600;
    border-radius: 6px; padding: 4px 10px; border: 1px solid var(--line-strong); }
  .agent { color: var(--accent); }
  .judge { color: var(--dim); }
  .k { font-size: 10px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.06em; opacity: 0.7; }
  .eff { font-weight: 500; opacity: 0.8; }
</style>
