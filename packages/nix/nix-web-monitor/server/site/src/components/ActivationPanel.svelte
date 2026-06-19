<script lang="ts">
  import { SvelteSet } from 'svelte/reactivity';
  import PanelHeader from '$lib/PanelHeader.svelte';
  import { formatDuration } from '$lib/format';
  import { useNow } from '$lib/now.svelte';
  import type { Activation, ActivationStep } from '$lib/types';

  type Props = {
    activation: Activation;
  };

  const { activation }: Props = $props();

  /// Live clock so the open step's duration ticks while it runs.
  const now = useNow();

  /// Steps the user expanded to read their captured output lines. Keyed by step
  /// name, which is unique within one activation run. `SvelteSet` is reactive, so
  /// in-place mutation re-renders.
  const expanded = new SvelteSet<string>();

  function toggle(name: string): void {
    if (expanded.has(name)) expanded.delete(name);
    else expanded.add(name);
  }

  const doneCount = $derived(activation.steps.filter((step) => step.status === 'done').length);

  function duration(step: ActivationStep): string {
    const end = step.stoppedAtMs ?? now.value;
    return formatDuration(Math.max(0, end - step.startedAtMs));
  }
</script>

<section class="panel activation-panel">
  <PanelHeader title="activation">
    {#if activation.steps.length > 0}
      <span class="panel-meta">{doneCount}/{activation.steps.length}</span>
    {/if}
  </PanelHeader>

  <div class="activation-body">
    {#if activation.steps.length === 0}
      <!-- A phase that produced no steps (still starting, or skipped because an
           earlier phase failed). The status explains why, so it never sits blank. -->
      <div class="activation-status">{activation.status || 'waiting for activation…'}</div>
    {:else}
      {#each activation.steps as step (step.name)}
        <div class="activation-step">
          <button
            class="activation-row"
            type="button"
            disabled={step.lines.length === 0}
            aria-expanded={expanded.has(step.name)}
            onclick={() => {
              toggle(step.name);
            }}
          >
            <span class="state" data-state={step.status} aria-hidden="true"></span>
            <span class="activation-name">{step.name}</span>
            {#if step.lines.length > 0}
              <span class="activation-lines-count">{step.lines.length}</span>
            {/if}
            <span class="activation-duration">{duration(step)}</span>
          </button>
          {#if expanded.has(step.name) && step.lines.length > 0}
            <pre class="activation-lines">{step.lines.join('\n')}</pre>
          {/if}
        </div>
      {/each}
    {/if}
  </div>
</section>
