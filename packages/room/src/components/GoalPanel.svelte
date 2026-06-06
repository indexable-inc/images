<script lang="ts">
  // Thread-scoped goal panel. The goal is the user's stable objective
  // for the thread — distinct from the per-turn plan rendered just
  // below by PlanPanel. Mirrors codex's `/goal` TUI surface.
  //
  // Empty state: a small "Set goal" affordance so the panel is
  // discoverable on a fresh thread without taking real space.
  // Active state: objective + (optional) token-budget progress bar +
  // an Edit / Clear menu.

  import * as api from '$lib/api';
  import type { ThreadGoal } from '$lib/types';

  interface Props {
    serverId: string;
    threadId: string;
    goal: ThreadGoal | null;
  }

  let { serverId, threadId, goal }: Props = $props();

  import { tick } from 'svelte';

  let editing = $state(false);
  let draft = $state('');
  let budgetDraft = $state('');
  let pending = $state(false);
  let error = $state<string | null>(null);
  let objectiveInput: HTMLInputElement | undefined = $state();

  async function startEdit() {
    draft = goal?.objective ?? '';
    budgetDraft = goal?.tokenBudget != null ? String(goal.tokenBudget) : '';
    error = null;
    editing = true;
    await tick();
    objectiveInput?.focus();
    objectiveInput?.select();
  }

  function cancelEdit() {
    editing = false;
    error = null;
  }

  async function submit() {
    const objective = draft.trim();
    if (!objective) {
      error = 'Objective required.';
      return;
    }
    const budget = budgetDraft.trim();
    const parsedBudget = budget.length > 0 ? Number(budget) : null;
    if (parsedBudget !== null && (!Number.isFinite(parsedBudget) || parsedBudget < 0)) {
      error = 'Budget must be a non-negative number.';
      return;
    }
    pending = true;
    error = null;
    try {
      await api.setThreadGoal(serverId, threadId, objective, parsedBudget);
      editing = false;
    } catch (err) {
      error = (err as Error).message;
    } finally {
      pending = false;
    }
  }

  async function clear() {
    pending = true;
    error = null;
    try {
      await api.clearThreadGoal(serverId, threadId);
    } catch (err) {
      error = (err as Error).message;
    } finally {
      pending = false;
    }
  }

  let budgetPct = $derived.by(() => {
    if (!goal || goal.tokenBudget == null || goal.tokenBudget <= 0) return null;
    const pct = (goal.tokensUsed / goal.tokenBudget) * 100;
    return Math.max(0, Math.min(100, pct));
  });

  function formatTokens(n: number): string {
    if (n < 1_000) return String(n);
    if (n < 1_000_000) return (n / 1_000).toFixed(1).replace(/\.0$/, '') + 'k';
    return (n / 1_000_000).toFixed(1).replace(/\.0$/, '') + 'M';
  }

  function formatDuration(seconds: number): string {
    if (seconds < 60) return `${Math.round(seconds)}s`;
    if (seconds < 3600) return `${Math.round(seconds / 60)}m`;
    const h = Math.floor(seconds / 3600);
    const m = Math.round((seconds % 3600) / 60);
    return m > 0 ? `${h}h ${m}m` : `${h}h`;
  }
</script>

{#if editing}
  <section class="goal editing" aria-label="Edit thread goal">
    <header class="goal-head">
      <span class="goal-title">Goal</span>
    </header>
    <input
      bind:this={objectiveInput}
      class="goal-objective-input"
      type="text"
      placeholder="What's the objective for this thread?"
      bind:value={draft}
      disabled={pending}
    />
    <input
      class="goal-budget-input"
      type="number"
      inputmode="numeric"
      min="0"
      placeholder="Token budget (optional)"
      bind:value={budgetDraft}
      disabled={pending}
    />
    {#if error}
      <p class="goal-error">{error}</p>
    {/if}
    <div class="goal-actions">
      <button type="button" class="goal-btn primary" onclick={submit} disabled={pending}>
        {pending ? 'Saving…' : 'Save'}
      </button>
      <button type="button" class="goal-btn" onclick={cancelEdit} disabled={pending}>
        Cancel
      </button>
    </div>
  </section>
{:else if goal}
  <section class="goal" aria-label="Thread goal" data-status={goal.status}>
    <header class="goal-head">
      <span class="goal-title">Goal</span>
      <span class="goal-status">{goal.status}</span>
      <span class="goal-meta">
        {formatTokens(goal.tokensUsed)}{#if goal.tokenBudget != null} / {formatTokens(goal.tokenBudget)} tok{:else} tok{/if}
        <span class="goal-sep">·</span>
        {formatDuration(goal.timeUsedSeconds)}
      </span>
      <span class="goal-actions inline">
        <button type="button" class="goal-btn ghost" onclick={startEdit} disabled={pending}>
          Edit
        </button>
        <button type="button" class="goal-btn ghost" onclick={clear} disabled={pending}>
          Clear
        </button>
      </span>
    </header>
    <p class="goal-objective">{goal.objective}</p>
    {#if budgetPct !== null}
      <div class="goal-bar" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow={budgetPct}>
        <div class="goal-bar-fill" style="width: {budgetPct}%"></div>
      </div>
    {/if}
    {#if error}
      <p class="goal-error">{error}</p>
    {/if}
  </section>
{:else}
  <section class="goal empty" aria-label="Set thread goal">
    <button type="button" class="goal-set" onclick={startEdit}>
      <span class="goal-title">Goal</span>
      <span class="goal-set-hint">Set a goal for this thread</span>
    </button>
  </section>
{/if}

<style>
  .goal {
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 18px 10px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-pane);
  }
  .goal.empty {
    padding: 4px 18px 4px;
  }
  .goal-head {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }
  .goal-title {
    font-variant: small-caps;
    letter-spacing: 0.04em;
    color: var(--text-muted);
    font-size: 11px;
  }
  .goal-status {
    color: var(--text-dim);
    font-size: 11px;
    text-transform: lowercase;
  }
  .goal-meta {
    color: var(--text-dim);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .goal-sep {
    margin: 0 2px;
    opacity: 0.6;
  }
  .goal-actions.inline {
    margin-left: auto;
    display: inline-flex;
    gap: 4px;
  }
  .goal-objective {
    margin: 0;
    color: var(--text);
    font-size: 13px;
    line-height: 1.4;
  }
  .goal-bar {
    height: 3px;
    border-radius: 2px;
    background: var(--bg-pill);
    overflow: hidden;
  }
  .goal-bar-fill {
    height: 100%;
    background: var(--accent, var(--text-muted));
    transition: width 0.18s ease;
  }
  .goal-set {
    display: inline-flex;
    align-items: baseline;
    gap: 8px;
    padding: 4px 0;
    color: var(--text-dim);
    font-size: 12px;
    cursor: pointer;
    background: none;
    border: none;
  }
  .goal-set:hover {
    color: var(--text);
  }
  .goal-set-hint {
    font-size: 12px;
  }
  .goal-objective-input,
  .goal-budget-input {
    background: var(--bg-elev);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 6px 8px;
    color: var(--text);
    font-size: 13px;
    font-family: inherit;
    outline: none;
  }
  .goal-objective-input:focus,
  .goal-budget-input:focus {
    border-color: var(--border-hi);
  }
  .goal-actions {
    display: flex;
    gap: 6px;
  }
  .goal-btn {
    padding: 4px 10px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--bg-elev);
    color: var(--text);
    font-size: 12px;
    cursor: pointer;
  }
  .goal-btn.primary {
    background: var(--accent, var(--text));
    color: var(--accent-text, var(--bg-pane));
    border-color: transparent;
  }
  .goal-btn.ghost {
    background: transparent;
    border-color: transparent;
    color: var(--text-dim);
    padding: 2px 6px;
    font-size: 11px;
  }
  .goal-btn.ghost:hover:not(:disabled) {
    color: var(--text);
    background: var(--bg-hover);
  }
  .goal-btn:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .goal-error {
    margin: 0;
    color: var(--danger, #c33);
    font-size: 11.5px;
  }
</style>
