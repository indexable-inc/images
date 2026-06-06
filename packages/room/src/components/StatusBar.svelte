<script lang="ts">
  // Thin status bar pinned to the bottom of the window, IntelliJ
  // style. Two zones:
  //
  //   left  → user-context segments that open drawers when clicked
  //           (Goal, Plan).
  //   right → system / agent info (agent work state, message count,
  //           model, connection state). Static — no drawers.
  //
  // Interactive segments use a button + hover bg; static segments
  // are flat text + tiny icon. Matches IntelliJ's "left side =
  // notifications/context, right side = passive info" convention.
  //
  // Empty / no-data segments stay visible where they make sense
  // (Goal: shows "Set goal"; Plan: hides when no plan; Model: hides
  // when null; Messages: hides when 0) so the affordances are
  // discoverable without crowding the bar with placeholders.

  import type { Thread } from '$lib/types';
  import GoalPanel from './GoalPanel.svelte';
  import PlanPanel from './PlanPanel.svelte';
  import WorkGlyph from './WorkGlyph.svelte';
  import Avatar from './Avatar.svelte';
  import { agentWorkMode } from '$lib/agentWork';
  import * as api from '$lib/api';
  import { nowTick } from '$lib/activity';
  import { roomFor } from '$lib/store';
  import { loadIdentity, type Identity } from '$lib/identity';
  import { openIdentity, identityOpen } from '$lib/ui';
  import IconTarget from '~icons/ph/target';
  import IconListChecks from '~icons/ph/list-checks';
  import IconCheck from '~icons/ph/check';
  import IconChats from '~icons/ph/chats';
  import IconCpu from '~icons/ph/cpu';
  import IconStop from '~icons/lucide/square';

  interface Props {
    serverId: string | null;
    /** Null on non-thread routes (not-found, the brief threads→new-chat
     *  redirect window). Drafts arrive as a real Thread with
     *  status='draft' but no server id; the goal/plan/work/messages
     *  segments are gated off below since there's no server thread to
     *  pin them to. */
    thread: Thread | null;
  }

  let { serverId, thread }: Props = $props();

  // A draft is a real Thread record but has no server-pinned goal,
  // plan, or message log yet. Treat it the same as "no thread" for the
  // purpose of the thread-scoped segments — the bar still renders, just
  // collapsed to identity + connection.
  let hasServerThread = $derived(thread !== null && thread.status !== 'draft');

  type Expanded = 'goal' | 'plan' | null;
  let expanded = $state<Expanded>(null);
  let stopping = $state(false);
  let stopError = $state<string | null>(null);

  function toggle(which: 'goal' | 'plan') {
    expanded = expanded === which ? null : which;
  }

  // Close any open drawer when the thread context disappears — leaving
  // an empty Goal drawer hanging over no-thread routes looks broken.
  $effect(() => {
    if (!hasServerThread) expanded = null;
  });
  $effect(() => {
    void thread?.id;
    stopError = null;
  });

  // First in-progress step, otherwise first pending. Returns null
  // when every step is complete — that's the "all done" state.
  let currentStep = $derived.by(() => {
    const plan = thread?.plan;
    if (!plan || plan.steps.length === 0) return null;
    const inProg = plan.steps.findIndex((s) => s.status === 'inProgress');
    if (inProg >= 0) return { text: plan.steps[inProg]!.step, idx: inProg };
    const next = plan.steps.findIndex((s) => s.status === 'pending');
    if (next >= 0) return { text: plan.steps[next]!.step, idx: next };
    return null;
  });

  let planMeta = $derived.by(() => {
    const plan = thread?.plan;
    if (!plan || plan.steps.length === 0) return null;
    const total = plan.steps.length;
    const done = plan.steps.filter((s) => s.status === 'completed').length;
    return { done, total };
  });

  let budgetPct = $derived.by(() => {
    const g = thread?.goal;
    if (!g || g.tokenBudget == null || g.tokenBudget <= 0) return null;
    return Math.max(0, Math.min(100, Math.round((g.tokensUsed / g.tokenBudget) * 100)));
  });

  // Wall-clock tick so the agent-state segment flips to null on
  // its own once a stuck-active thread crosses the staleness window.
  let nowMs = $state(Date.now());
  $effect(() => nowTick.subscribe((v) => (nowMs = v)));

  let workState = $derived(thread ? agentWorkMode(thread, nowMs) : null);

  async function stopThread() {
    if (!serverId || !thread || stopping) return;
    stopping = true;
    stopError = null;
    try {
      await api.interruptThread(serverId, thread.id);
    } catch (err) {
      stopError = err instanceof Error ? err.message : String(err);
    } finally {
      stopping = false;
    }
  }

  // WebSocket state, surfaced once in the global status bar so the
  // sidebar can stay focused on navigation.
  let connection = $state<'connecting' | 'open' | 'closed'>('connecting');
  $effect(() => {
    if (!serverId) {
      connection = 'closed';
      return;
    }
    return roomFor(serverId).connection.subscribe((v) => (connection = v));
  });

  let connectionLabel = $derived(
    connection === 'open' ? 'connected' : connection === 'connecting' ? 'reconnecting' : 'disconnected'
  );

  // Local identity, surfaced as a tiny avatar at the leftmost
  // position. Re-reads on every roomDoc presence tick so it picks up
  // the change immediately after the user saves a new name in the
  // identity modal. (Cheaper than wiring a dedicated store — the
  // presence stream already ticks any time we setSelf.)
  let me = $state<Identity>(loadIdentity());
  $effect(() => {
    if (!serverId) return;
    return roomFor(serverId).doc.presenceList.subscribe(() => (me = loadIdentity()));
  });
  // Also bump when the modal closes, since modal-close may run
  // before presence ticks back.
  $effect(() => identityOpen.subscribe(() => (me = loadIdentity())));

  let meTitle = $derived(
    me.kind === 'github' && me.github
      ? `${me.name} (@${me.github})`
      : me.name
  );
</script>

<div class="status-wrap">
  {#if hasServerThread && thread && serverId && expanded === 'goal'}
    <div class="drawer">
      <GoalPanel {serverId} threadId={thread.id} goal={thread.goal} />
    </div>
  {:else if hasServerThread && thread?.plan && expanded === 'plan'}
    <div class="drawer">
      <PlanPanel plan={thread.plan} />
    </div>
  {/if}

  <div class="bar" role="toolbar" aria-label="Status">
    <button
      type="button"
      class="me"
      onclick={openIdentity}
      title={meTitle}
      aria-label="Set identity"
    >
      <Avatar name={me.name} github={me.github ?? null} size={14} />
    </button>

    {#if hasServerThread && thread}
      <button
        type="button"
        class="seg goal"
        class:active={expanded === 'goal'}
        onclick={() => toggle('goal')}
        aria-expanded={expanded === 'goal'}
        title={thread.goal ? thread.goal.objective : 'Set a goal for this thread'}
      >
        <span class="ico" aria-hidden="true"><IconTarget width={12} height={12} /></span>
        <span class="label" class:muted={!thread.goal}>
          {thread.goal ? thread.goal.objective : 'Set goal'}
        </span>
        {#if budgetPct !== null}
          <span class="meta">{budgetPct}%</span>
        {/if}
      </button>
    {/if}

    <span class="spacer"></span>

    {#if hasServerThread && planMeta}
      <button
        type="button"
        class="seg plan"
        class:active={expanded === 'plan'}
        onclick={() => toggle('plan')}
        aria-expanded={expanded === 'plan'}
        title={currentStep ? currentStep.text : 'All steps complete'}
      >
        <span class="ico" aria-hidden="true">
          {#if currentStep}
            <IconListChecks width={12} height={12} />
          {:else}
            <IconCheck width={12} height={12} />
          {/if}
        </span>
        <span class="label">
          {currentStep ? currentStep.text : 'Plan complete'}
        </span>
        <span class="meta">{planMeta.done}/{planMeta.total}</span>
      </button>
    {/if}

    {#if hasServerThread && workState !== null}
      <span class="seg static work" title={workState === 'working' ? 'Working' : 'Awaiting input'}>
        <span class="ico" aria-hidden="true"><WorkGlyph mode={workState} /></span>
        <span class="label">{workState === 'working' ? 'Working' : 'Awaiting input'}</span>
      </span>
    {/if}

    {#if hasServerThread && thread && serverId && workState === 'working'}
      <button
        type="button"
        class="seg stop"
        disabled={stopping}
        onclick={stopThread}
        title={stopError ?? (stopping ? 'Stopping current turn' : 'Stop current turn')}
        aria-label="Stop current turn"
      >
        <span class="ico" aria-hidden="true"><IconStop width={11} height={11} /></span>
        <span class="label">{stopping ? 'Stopping' : 'Stop'}</span>
      </button>
    {/if}

    {#if hasServerThread && thread && thread.message_count > 0}
      <span class="seg static" title="{thread.message_count} message{thread.message_count === 1 ? '' : 's'}">
        <span class="ico" aria-hidden="true"><IconChats width={12} height={12} /></span>
        <span class="meta">{thread.message_count}</span>
      </span>
    {/if}

    {#if hasServerThread && thread?.model}
      <span class="seg static" title="Model: {thread.model}">
        <span class="ico" aria-hidden="true"><IconCpu width={12} height={12} /></span>
        <span class="label">{thread.model}</span>
      </span>
    {/if}

    <span class="conn" data-state={connection} title={connectionLabel} aria-label={connectionLabel}>
      <span class="conn-dot"></span>
      <span class="conn-label">{connectionLabel}</span>
    </span>
  </div>
</div>

<style>
  .status-wrap {
    display: flex;
    flex-direction: column;
    flex-shrink: 0;
    /* Stronger top divider so the bar reads as a distinct surface
       from the pane above instead of merging into the composer's
       bottom padding. */
    border-top: 1px solid var(--border-hi);
  }
  /* The drawer is the existing GoalPanel / PlanPanel mounted inline.
     A second border at the bottom separates the drawer from the bar
     so they read as two zones rather than one blob. */
  .drawer {
    border-bottom: 1px solid var(--border);
    background: var(--bg-pane);
  }
  /* IntelliJ-style: thin row, very small font, flat segments with
     subtle hover. Interactive segments (goal, plan) get a hover bg;
     static segments (work state, messages, model) are flat with no
     cursor change. */
  .bar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 0 14px;
    height: 28px;
    background: transparent;
    font-size: 11px;
    color: var(--text-dim);
  }
  .spacer {
    flex: 1;
  }
  /* "Me" avatar segment at the leftmost position. No text label —
     the avatar is the affordance; full identity surfaces on hover
     via the title attribute. */
  .me {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 14px;
    height: 14px;
    padding: 0;
    background: transparent;
    border-radius: 999px;
    cursor: pointer;
    flex-shrink: 0;
    transition: filter 0.08s;
  }
  .me:hover {
    filter: brightness(1.15);
  }
  .seg {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    max-width: 50%;
    padding: 2px 4px;
    margin: 0 -4px;
    border-radius: 3px;
    background: transparent;
    color: var(--text-muted);
    font-size: 11px;
    line-height: 1;
    cursor: pointer;
    overflow: hidden;
    transition: background 0.08s, color 0.08s;
  }
  .seg:hover:not(.static) {
    background: var(--bg-hover);
    color: var(--text);
  }
  .seg.stop {
    color: var(--danger);
  }
  .seg.stop:hover:not(:disabled) {
    background: color-mix(in srgb, var(--danger) 14%, transparent);
    color: var(--danger);
  }
  .seg.stop:disabled {
    cursor: default;
    opacity: 0.65;
  }
  .seg.stop .ico {
    color: currentColor;
  }
  .seg.active {
    background: var(--bg-active);
    color: var(--text-strong);
  }
  .seg.static {
    cursor: default;
    max-width: 30%;
  }
  .ico {
    display: inline-flex;
    align-items: center;
    color: var(--text-dim);
    flex-shrink: 0;
  }
  .seg:hover:not(.static) .ico,
  .seg.active .ico {
    color: var(--text);
  }
  .seg.static.work .ico {
    /* WorkGlyph manages its own color */
    color: inherit;
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .label.muted {
    color: var(--text-dim);
  }
  .meta {
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
    font-size: 11px;
    flex-shrink: 0;
  }

  /* Connection dot, far right. 8x8 with a 1px ring matches the
     density of IntelliJ's notification + lock indicators. Color
     comes from the data-state attribute. */
  .conn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 14px;
    flex-shrink: 0;
    cursor: default;
    min-width: 0;
  }
  .conn-label {
    color: var(--text-dim);
    font-size: 11px;
    line-height: 1;
    white-space: nowrap;
    transform: translateY(-0.5px);
  }
  .conn-dot {
    width: 8px;
    height: 8px;
    border-radius: 999px;
    background: var(--text-dim);
    box-shadow: 0 0 0 1px var(--bg-pane);
  }
  .conn[data-state='open'] .conn-dot {
    background: #34c759;
  }
  .conn[data-state='open'] .conn-label {
    color: #34c759;
  }
  .conn[data-state='connecting'] .conn-dot {
    background: #ff9f0a;
    animation: conn-pulse 1.2s ease-in-out infinite;
  }
  .conn[data-state='connecting'] .conn-label {
    color: #ff9f0a;
  }
  .conn[data-state='closed'] .conn-dot {
    background: #ff453a;
  }
  .conn[data-state='closed'] .conn-label {
    color: #ff453a;
  }
  @keyframes conn-pulse {
    0%, 100% { opacity: 0.45; }
    50%      { opacity: 1; }
  }
</style>
