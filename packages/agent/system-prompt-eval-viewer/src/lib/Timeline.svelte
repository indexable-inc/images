<script lang="ts">
  import type { Step as StepT } from './types';
  import Step from './steps/Step.svelte';
  import CodeBlock from './ui/CodeBlock.svelte';
  let { steps, transcript }: { steps?: StepT[]; transcript?: string } = $props();
</script>

{#if steps && steps.length}
  <div class="timeline">
    <div class="count">{steps.length} steps</div>
    {#each steps as s}
      <Step step={s} />
    {/each}
  </div>
{:else if transcript}
  <CodeBlock text={transcript} variant="out" />
{:else}
  <p class="muted">no transcript captured</p>
{/if}

<style>
  .timeline { padding-left: 2px; }
  .count { font-size: 11px; color: var(--dim); text-transform: uppercase; letter-spacing: 0.06em; margin: 2px 0 8px; }
  .muted { color: var(--dim); }
</style>
