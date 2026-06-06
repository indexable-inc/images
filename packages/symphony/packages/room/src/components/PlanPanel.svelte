<script lang="ts">
  import type { ThreadPlan } from '$lib/types';

  interface Props {
    plan: ThreadPlan;
  }

  let { plan }: Props = $props();

  const total = $derived(plan.steps.length);
  const done = $derived(plan.steps.filter((s) => s.status === 'completed').length);
</script>

{#if total > 0}
  <section class="plan" aria-label="Agent plan">
    <header class="plan-head">
      <span class="plan-title">Plan</span>
      <span class="plan-count" aria-label="{done} of {total} steps complete">
        {done}/{total}
      </span>
    </header>
    {#if plan.explanation}
      <p class="plan-explanation">{plan.explanation}</p>
    {/if}
    <ol class="plan-steps">
      {#each plan.steps as step, i (i)}
        <li class="plan-step" data-status={step.status}>
          <span class="plan-marker" aria-hidden="true">
            {#if step.status === 'completed'}
              ✓
            {:else if step.status === 'inProgress'}
              ◐
            {:else}
              ○
            {/if}
          </span>
          <span class="plan-text">{step.step}</span>
        </li>
      {/each}
    </ol>
  </section>
{/if}

<style>
  .plan {
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 18px 10px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-pane);
    max-height: 220px;
    overflow-y: auto;
  }
  .plan-head {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }
  .plan-title {
    font-variant: small-caps;
    letter-spacing: 0.04em;
    color: var(--text-muted);
    font-size: 11px;
  }
  .plan-count {
    color: var(--text-dim);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .plan-explanation {
    margin: 0;
    color: var(--text);
    font-size: 13px;
    line-height: 1.4;
  }
  .plan-steps {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .plan-step {
    display: grid;
    grid-template-columns: 14px 1fr;
    gap: 8px;
    align-items: baseline;
    font-size: 13px;
    line-height: 1.35;
    color: var(--text);
  }
  .plan-marker {
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
    text-align: center;
  }
  .plan-step[data-status='completed'] .plan-text {
    color: var(--text-muted);
    text-decoration: line-through;
  }
  .plan-step[data-status='completed'] .plan-marker {
    color: var(--text-muted);
  }
  .plan-step[data-status='inProgress'] .plan-marker {
    color: var(--accent, var(--text));
  }
  .plan-step[data-status='inProgress'] .plan-text {
    font-weight: 600;
  }
</style>
