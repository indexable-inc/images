<script lang="ts">
  import type { Step } from '../types';
  import StepMessage from './StepMessage.svelte';
  import StepToolUse from './StepToolUse.svelte';
  import StepToolResult from './StepToolResult.svelte';
  import StepThinking from './StepThinking.svelte';

  let { step }: { step: Step } = $props();

  const icon: Record<string, string> = {
    text: '\u{1F4AC}',
    thinking: '\u{1F914}',
    tool_use: '\u{1F527}',
    tool_result: '\u{2192}',
    final: '\u{2714}',
  };
</script>

<div class="step {step.kind}" class:err={step.is_error}>
  <div class="rail"><span class="ico">{icon[step.kind] ?? '\u{2022}'}</span></div>
  <div class="body">
    {#if step.kind === 'tool_use'}
      <StepToolUse name={step.name} input={step.input} />
    {:else if step.kind === 'tool_result'}
      <StepToolResult text={step.text} isError={step.is_error} />
    {:else if step.kind === 'thinking'}
      <StepThinking text={step.text} />
    {:else if step.kind === 'final'}
      <StepMessage text={step.text} final />
    {:else}
      <StepMessage text={step.text} />
    {/if}
  </div>
</div>

<style>
  .step { display: grid; grid-template-columns: 26px 1fr; gap: 10px; margin: 10px 0; }
  .rail { display: flex; justify-content: center; }
  .ico { width: 26px; height: 26px; border-radius: var(--radius); display: grid; place-items: center;
    background: var(--chip); font-size: 13px; border: 1px solid var(--line); }
  .step.tool_use .ico { background: color-mix(in oklab, var(--accent) 14%, var(--chip)); }
  .step.final .ico { background: var(--good-bg); }
  .step.err .ico { background: var(--bad-bg); }
  .body { min-width: 0; }
</style>
