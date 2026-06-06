<script lang="ts">
  // Thread detail pane. Mirrors the Claude Code app layout: thin
  // header at the top, scrollable transcript in the middle, live
  // chat composer pinned to the bottom.

  import { onDestroy, tick, untrack } from 'svelte';
  import { get } from 'svelte/store';
  import { roomFor } from '$lib/store';
  import * as api from '$lib/api';
  import type { Message, Thread } from '$lib/types';
  import MessageUser from '$components/MessageUser.svelte';
  import MessageAssistant from '$components/MessageAssistant.svelte';
  import MessageSystem from '$components/MessageSystem.svelte';
  import ToolWork from '$components/ToolWork.svelte';
  import Composer from '$components/Composer.svelte';
  import ReadingPositions from '$components/ReadingPositions.svelte';
  import ChatFind from '$components/ChatFind.svelte';
  import ThreadSidePanel from '$components/ThreadSidePanel.svelte';
  import WorkGlyph from '$components/WorkGlyph.svelte';
  import IconPullRequest from '~icons/ph/git-pull-request';
  import IconIssue from '~icons/ph/circle';
  import IconPanelRightClose from '~icons/lucide/panel-right-close';
  import IconPanelRightOpen from '~icons/lucide/panel-right-open';
  import { loadIdentity } from '$lib/identity';
  import { extractIssues } from '$lib/issues';
  import { drafts, draftAsThread } from '$lib/drafts';
  import { sidebarActive } from '$lib/ui';
  import { rightPanelOpen, rightPanelTab, toggleThreadPanel, type ThreadPanelTab } from '$lib/threadPanels';
  import { nowTick } from '$lib/activity';
  import { agentWorkMode } from '$lib/agentWork';
  import { loadingLine } from '$lib/loadingCopy';

  interface Props {
    serverId: string;
    threadId: string;
  }

  let { serverId, threadId }: Props = $props();
  let currentRoom = $derived(roomFor(serverId));
  let roomDoc = $derived(currentRoom.doc);

  let thread = $state<Thread | undefined>(undefined);
  let messages = $state<Message[]>([]);
  let loadError = $state<string | null>(null);
  let scroller: HTMLElement | undefined = $state();
  let pinnedToBottom = $state(true);
  let findOpen = $state(false);
  let findFocusBump = $state(0);
  let panelOpen = $state(false);
  let panelTab = $state<ThreadPanelTab>('review');
  let nowMs = $state(Date.now());
  $effect(() => nowTick.subscribe((v) => (nowMs = v)));
  $effect(() => rightPanelOpen.subscribe((v) => (panelOpen = v)));
  $effect(() => rightPanelTab.subscribe((v) => (panelTab = v)));
  let workState = $derived(agentWorkMode(thread, nowMs));
  let workLabel = $derived(
    workState === 'waiting' ? 'Agent is waiting for input' : 'Agent is responding'
  );
  let loadingText = $derived(
    workState === 'working' ? loadingLine(threadId, nowMs) : workLabel
  );

  // Everything below keys off the routed server + thread pair so navigating between
  // chats updates this component in place instead of remounting it.
  // Each $effect's cleanup runs before the effect re-runs, which
  // unsubscribes the previous stores cleanly.

  // Reset transient view state on every switch — without this we'd
  // briefly display the previous thread's data while subscriptions
  // resync.
  $effect(() => {
    void serverId;
    void threadId;
    untrack(() => {
      thread = undefined;
      messages = [];
      loadError = null;
      pinnedToBottom = true;
      findOpen = false;
      if (scrollPctTimer) {
        clearTimeout(scrollPctTimer);
        scrollPctTimer = null;
      }
      pendingScrollPct = null;
      pendingViewportPct = null;
      pendingScrollFor = null;
      // The de-dup memo is per-thread — clearing it forces the first
      // broadcast on the new thread to actually go through.
      lastSentScrollPct = null;
      lastSentViewportPct = null;
    });
  });

  // Resolve the thread. Server cache wins; otherwise surface the local
  // draft so the header + composer render immediately. Re-running the
  // subscription handler keeps the title in sync as the user types
  // into the draft composer.
  //
  // The inner reads of `thread` go through `untrack` because the
  // subscribers also *write* to it — without untrack Svelte adds
  // `thread` as a dep of this effect, then the write triggers a
  // re-run, which re-subscribes, which writes again. Infinite loop.
  $effect(() => {
    const sid = serverId;
    const id = threadId;
    const room = currentRoom;
    const unsubT = room.threads.subscribe((m) => {
      const t = m.get(id);
      if (t) thread = t;
    });
    const unsubD = drafts.subscribe((m) => {
      const isServerResolved = untrack(() => thread && thread.status !== 'draft');
      if (isServerResolved) return;
      const d = m.get(id);
      if (d && d.server_id !== serverId) return;
      if (d) thread = draftAsThread(d, currentRoom.server.name);
    });
    return () => {
      unsubT();
      unsubD();
    };
  });

  // Messages stream for the active thread. `pinnedToBottom` is also
  // untracked — onScroll writes it, and we don't want this effect to
  // resubscribe every time the user scrolls.
  $effect(() => {
    const sid = serverId;
    const id = threadId;
    const room = currentRoom;
    const sub = room.messagesFor(id).subscribe((list) => {
      if (list) {
        messages = list;
        if (untrack(() => pinnedToBottom)) void tick().then(scrollToBottom);
      }
    });
    void room.ensureMessages(id);
    return sub;
  });

  // Fallback to an HTTP fetch when neither server cache nor draft has
  // the id. The `id === threadId` guard inside the promise handlers
  // protects against a slow fetch landing after the user has already
  // navigated away.
  $effect(() => {
    const sid = serverId;
    const id = threadId;
    const room = currentRoom;
    const alreadyHave = untrack(
      () =>
        thread !== undefined ||
        get(drafts).get(id)?.server_id === serverId ||
        get(room.threads).has(id)
    );
    if (alreadyHave) return;
    api
      .getThread(sid, id)
      .then((fetched) => {
        if (fetched && sid === serverId && id === threadId) thread = fetched;
      })
      .catch((err) => {
        if (sid === serverId && id === threadId) loadError = (err as Error).message;
      });
  });

  // Presence: announce viewing on switch with scroll_pct=0 so a peer
  // who has just opened the thread (and hasn't scrolled yet) still
  // shows up at the top of the gutter — the gutter is now the only
  // place we render presence in the thread, so silence here would
  // hide them entirely.
  $effect(() => {
    const id = threadId;
    const self = loadIdentity();
    const doc = roomDoc;
    doc.setSelf(self, {
      online: true,
      viewing_thread_id: id,
      typing_thread_id: null,
      scroll_pct: 0
    });
  });

  onDestroy(() => {
    if (scrollPctTimer) {
      clearTimeout(scrollPctTimer);
      scrollPctTimer = null;
    }
    pendingScrollPct = null;
    pendingScrollFor = null;
    const self = loadIdentity();
    roomDoc.setSelf(self, {
      online: true,
      viewing_thread_id: null,
      typing_thread_id: null,
      scroll_pct: null
    });
  });

  function scrollToBottom() {
    if (!scroller) return;
    scroller.scrollTop = scroller.scrollHeight;
  }

  // Vim half-page scroll. ⌃u / ⌃d are the canonical bindings; we also
  // accept bare `k` / `j` (up / down) as long as the user isn't typing
  // in a text field, so the chat reads as a "viewer mode" surface like
  // nerdtree. `g` / `G` jump to top / bottom for parity with the sidebar.
  function isInTextField(target: EventTarget | null): boolean {
    if (!(target instanceof HTMLElement)) return false;
    if (target.isContentEditable) return true;
    const tag = target.tagName;
    return tag === 'INPUT' || tag === 'TEXTAREA';
  }

  // Spring-chase smooth scroll. Each animation frame moves the scroll
  // position a fraction of the remaining distance to the target —
  // gives an exponential ease-out that feels natural without explicit
  // easing curves.
  //
  // Rapid j/j/j presses compound the target instead of restarting;
  // the animation keeps chasing the new (further) target seamlessly,
  // so holding the key tracks the same trajectory whether you tap
  // once or ten times in a row.
  //
  // FACTOR controls "tightness" — at 60fps, 0.18 settles in ~25
  // frames (~400ms) to within a pixel. Lower = silkier but slower
  // to commit; higher = snappier but more obviously stepped.
  const SCROLL_CHASE_FACTOR = 0.18;
  let scrollTarget: number | null = null;
  let scrollAnimId: number | null = null;

  function setScrollTarget(value: number) {
    if (!scroller) return;
    const max = scroller.scrollHeight - scroller.clientHeight;
    scrollTarget = Math.max(0, Math.min(max, value));
    if (scrollAnimId === null) {
      scrollAnimId = requestAnimationFrame(stepScroll);
    }
  }

  function stepScroll() {
    if (!scroller || scrollTarget === null) {
      scrollAnimId = null;
      scrollTarget = null;
      return;
    }
    const current = scroller.scrollTop;
    const remaining = scrollTarget - current;
    if (Math.abs(remaining) < 0.5) {
      scroller.scrollTop = scrollTarget;
      scrollAnimId = null;
      scrollTarget = null;
      return;
    }
    scroller.scrollTop = current + remaining * SCROLL_CHASE_FACTOR;
    scrollAnimId = requestAnimationFrame(stepScroll);
  }

  function cancelScrollAnim() {
    if (scrollAnimId !== null) {
      cancelAnimationFrame(scrollAnimId);
      scrollAnimId = null;
    }
    scrollTarget = null;
  }

  function halfPageScroll(direction: 1 | -1) {
    if (!scroller) return;
    const baseline = scrollTarget ?? scroller.scrollTop;
    setScrollTarget(baseline + scroller.clientHeight * 0.5 * direction);
  }
  function jumpToTop() {
    setScrollTarget(0);
  }

  // User-driven wheel / touch interrupts our chase so we never fight
  // a real input. Only fires on actual input (our scrollTop writes
  // don't emit wheel events), so no flag-juggling needed.
  $effect(() => {
    if (!scroller) return;
    const el = scroller;
    const cancel = () => cancelScrollAnim();
    el.addEventListener('wheel', cancel, { passive: true });
    el.addEventListener('touchstart', cancel, { passive: true });
    return () => {
      el.removeEventListener('wheel', cancel);
      el.removeEventListener('touchstart', cancel);
    };
  });

  // Cmd-F / Ctrl-F opens the in-chat find bar. Kept separate from
  // the viewer-mode handler below because that one short-circuits on
  // any meta key, and we want ⌘F to fire even when the composer or
  // another text field owns focus (browser-find affordance).
  $effect(() => {
    function onFindKey(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && (e.key === 'f' || e.key === 'F')) {
        e.preventDefault();
        findOpen = true;
        findFocusBump++;
      }
    }
    window.addEventListener('keydown', onFindKey);
    return () => window.removeEventListener('keydown', onFindKey);
  });

  $effect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.metaKey || e.altKey) return;
      if (isInTextField(e.target)) return;
      // Sidebar owns the keyboard while focused — let it consume j/k
      // /g/G/Enter for chat-list navigation.
      if (get(sidebarActive)) return;
      // Esc in the transcript hands focus back to the composer so
      // the user can toggle between scrolling the contents and
      // typing without the mouse. Counterpart to Composer's Esc,
      // which blurs the textarea and drops the user into this
      // window-level vim layer.
      if (e.key === 'Escape') {
        const ta = document.querySelector<HTMLTextAreaElement>('.composer textarea');
        if (ta) {
          e.preventDefault();
          ta.focus();
        }
        return;
      }
      if (e.ctrlKey) {
        if (e.key === 'd' || e.key === 'D') {
          e.preventDefault();
          halfPageScroll(1);
          return;
        }
        if (e.key === 'u' || e.key === 'U') {
          e.preventDefault();
          halfPageScroll(-1);
          return;
        }
        return;
      }
      if (e.key === 'j') {
        e.preventDefault();
        halfPageScroll(1);
      } else if (e.key === 'k') {
        e.preventDefault();
        halfPageScroll(-1);
      } else if (e.key === 'g') {
        e.preventDefault();
        jumpToTop();
      } else if (e.key === 'G') {
        e.preventDefault();
        if (scroller) setScrollTarget(scroller.scrollHeight);
      }
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  // Click on a remote peer's gutter bubble lands us at roughly their
  // scroll position. Smooth-scroll so the jump reads as a guided
  // motion instead of an abrupt teleport. The pct is fraction down
  // the SCROLL RANGE, so scrollTop = pct * (scrollHeight - clientHeight).
  function jumpToScrollPct(pct: number) {
    if (!scroller) return;
    const range = scroller.scrollHeight - scroller.clientHeight;
    if (range <= 0) return;
    const target = Math.max(0, Math.min(range, pct * range));
    scroller.scrollTo({ top: target, behavior: 'smooth' });
  }

  // Fire one onScroll synthesis after the transcript has rendered so
  // remote peers immediately see this viewer's initial viewport range
  // (instead of having to wait for the user to actually scroll).
  $effect(() => {
    void threadId;
    void messages;
    void tick().then(() => onScroll());
  });

  // Throttle scroll-position broadcasts. 80ms (~12 Hz) is the sweet
  // spot — fast enough that individual jumps stay below the
  // eye-noticeable threshold, and the CSS transition on the receiver
  // (also 80ms, linear) smoothly chains step-to-step so motion looks
  // continuous instead of jolting between samples. `pendingScrollFor`
  // snaps the threadId at scroll time so a delayed timer firing
  // after a thread switch doesn't broadcast a stale pct against the
  // new thread.
  const SCROLL_BROADCAST_MS = 80;
  let scrollPctTimer: ReturnType<typeof setTimeout> | null = null;
  let pendingScrollPct: number | null = null;
  let pendingViewportPct: number | null = null;
  let pendingScrollFor: string | null = null;
  let lastSentScrollPct: number | null = null;
  let lastSentViewportPct: number | null = null;
  function flushScrollPct() {
    scrollPctTimer = null;
    if (pendingScrollPct === null || pendingScrollFor === null) return;
    const pct = pendingScrollPct;
    const viewport = pendingViewportPct;
    const targetId = pendingScrollFor;
    pendingScrollPct = null;
    pendingViewportPct = null;
    pendingScrollFor = null;
    // Stale fire after a thread switch — drop instead of broadcasting
    // for the wrong thread.
    if (targetId !== threadId) return;
    // Skip if nothing meaningful changed. 0.001 (0.1%) is well below
    // a pixel of bubble motion at typical heights and saves chatter
    // when the user is idle but the timer keeps re-arming for any
    // reason.
    if (
      lastSentScrollPct !== null &&
      lastSentViewportPct !== null &&
      Math.abs(pct - lastSentScrollPct) < 0.001 &&
      Math.abs((viewport ?? 0) - lastSentViewportPct) < 0.001
    ) {
      return;
    }
    lastSentScrollPct = pct;
    lastSentViewportPct = viewport;
    const self = loadIdentity();
    // Don't re-publish viewing_thread_id from a scroll: the mount
    // effect above already set it, and setSelf's merge preserves it.
    // Two windows of the same identity share a single presence slot,
    // and writing viewing_thread_id from scroll causes them to fight
    // over the slot — peers then see this user's avatar flickering
    // between the two threads' sidebar rows. Scroll pct/viewport are
    // the only per-frame facts to publish here.
    roomDoc.setSelf(self, {
      online: true,
      scroll_pct: pct,
      viewport_pct: viewport
    });
  }

  function onScroll() {
    if (!scroller) return;
    const fromBottom = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    pinnedToBottom = fromBottom < 80;

    const total = scroller.scrollHeight;
    const visible = scroller.clientHeight;
    const range = total - visible;
    const pct = range > 0 ? Math.max(0, Math.min(1, scroller.scrollTop / range)) : 0;
    // Fraction of the transcript that's currently on screen. Clamps
    // to (0,1]; a one-screen-or-smaller thread is 1.0 (peer sees
    // everything), a very long thread might be 0.05 (5% visible).
    const viewport = total > 0 ? Math.max(0.02, Math.min(1, visible / total)) : 1;
    pendingScrollPct = pct;
    pendingViewportPct = viewport;
    pendingScrollFor = threadId;
    if (!scrollPctTimer) scrollPctTimer = setTimeout(flushScrollPct, SCROLL_BROADCAST_MS);
  }

  type Block =
    | { kind: 'user'; message: Message }
    | { kind: 'assistant'; message: Message }
    | { kind: 'system'; message: Message }
    | { kind: 'work'; messages: Message[] };

  let blocks = $derived(groupBlocks(messages));
  let issues = $derived(extractIssues(messages, thread?.repo ?? null));
  function groupBlocks(msgs: Message[]): Block[] {
    const out: Block[] = [];
    let pending: Message[] = [];
    const flush = () => {
      if (pending.length > 0) {
        out.push({ kind: 'work', messages: pending });
        pending = [];
      }
    };
    for (const m of msgs) {
      // Thinking traces bundle into the same collapsible "Show Work"
      // group as the adjacent tool calls so a turn's reasoning + actions
      // read together when expanded.
      if (
        m.kind === 'thinking' ||
        m.role === 'tool' ||
        m.kind === 'tool_call' ||
        m.kind === 'tool_result'
      ) {
        pending.push(m);
        continue;
      }
      flush();
      if (m.role === 'user') out.push({ kind: 'user', message: m });
      else if (m.role === 'assistant') out.push({ kind: 'assistant', message: m });
      else out.push({ kind: 'system', message: m });
    }
    flush();
    return out;
  }

</script>

{#if loadError}
  <div class="state error">Could not load thread: {loadError}</div>
{:else if !thread}
  <div class="state loading">Loading thread&hellip;</div>
{:else}
  <header class="head">
    <div class="title-block">
      {#if issues.length > 0}
        <span class="issues">
          {#each issues.slice(0, 6) as ref (ref.url)}
            <a
              class="issue-chip"
              class:pr={ref.kind === 'pull'}
              href={ref.url}
              target="_blank"
              rel="noopener noreferrer"
              title={`${ref.owner}/${ref.repo}#${ref.number} - ${ref.kind === 'pull' ? 'pull request' : 'issue'}`}
            >
              {#if ref.kind === 'pull'}
                <IconPullRequest class="issue-icon" />
              {:else}
                <IconIssue class="issue-icon" />
              {/if}
              <span>{ref.label}</span>
            </a>
          {/each}
          {#if issues.length > 6}
            <span class="issue-more">+{issues.length - 6}</span>
          {/if}
        </span>
      {/if}
    </div>
    <button
      type="button"
      class="inspector-toggle"
      class:active={panelOpen}
      onclick={() => toggleThreadPanel('review')}
      aria-pressed={panelOpen}
      title={panelOpen ? 'Hide side panel' : 'Show side panel'}
    >
      {#if panelOpen}
        <IconPanelRightClose width={13} height={13} />
      {:else}
        <IconPanelRightOpen width={13} height={13} />
      {/if}
      <span>Review</span>
    </button>
  </header>

  <div class="workbench">
    <div class="transcript-wrap">
      <ReadingPositions {serverId} {threadId} onJumpTo={jumpToScrollPct} />
      <ChatFind
        open={findOpen}
        {scroller}
        focusBump={findFocusBump}
        onClose={() => (findOpen = false)}
      />
      <section class="transcript" bind:this={scroller} onscroll={onScroll}>
        <div class="inner">
        {#if blocks.length === 0}
          {#if workState}
            <div class="agent-loading empty">
              <WorkGlyph mode={workState} size={14} label={workLabel} />
              <span>{loadingText}&hellip;</span>
            </div>
          {:else}
            <div class="state placeholder">Type a message to begin&hellip;</div>
          {/if}
        {:else}
          {#each blocks as block, i (i)}
            {#if block.kind === 'user'}
              <MessageUser message={block.message} />
            {:else if block.kind === 'assistant'}
              <MessageAssistant {serverId} message={block.message} />
            {:else if block.kind === 'system'}
              <MessageSystem message={block.message} />
            {:else if block.kind === 'work'}
              <ToolWork {serverId} messages={block.messages} />
            {/if}
          {/each}
          {#if workState}
            <div class="agent-loading">
              <WorkGlyph mode={workState} size={14} label={workLabel} />
              <span>{loadingText}&hellip;</span>
            </div>
          {/if}
        {/if}
        </div>
      </section>
    </div>
    {#if panelOpen}
      <ThreadSidePanel {serverId} {threadId} tab={panelTab} onTab={(tab) => rightPanelTab.set(tab)} />
    {/if}
  </div>

  <!-- Keyed so the composer's draft-text seed and focus runs per
       thread switch even though ThreadDetail itself stays mounted. -->
  {#key serverId + ':' + threadId}
    <Composer {serverId} {threadId} />
  {/key}
{/if}

<style>
  /* Title is already shown in the sidebar — the header just hosts
     issue chips (when present). Per-viewer presence is rendered by
     the gutter (ReadingPositions), so the header collapses when
     there are no chips. */
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 18px;
    height: 42px;
    background: var(--bg-pane);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .title-block {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    flex: 1;
  }
  .inspector-toggle {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 22px;
    padding: 0 2px;
    border: 0;
    color: var(--text-dim);
    background: transparent;
    font-size: 11px;
    cursor: pointer;
    flex-shrink: 0;
  }
  .inspector-toggle:hover,
  .inspector-toggle.active {
    color: var(--text);
  }
  .inspector-toggle :global(svg) {
    flex-shrink: 0;
  }
  .issues {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
  }
  .issue-chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 1px 7px 1px 5px;
    border-radius: 999px;
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 1.4;
    text-decoration: none;
    transition: background 0.12s, color 0.12s;
  }
  .issue-chip:hover {
    background: var(--bg-pill);
    color: var(--text-strong);
  }
  .issue-chip :global(.issue-icon) {
    width: 12px;
    height: 12px;
    color: var(--text-dim);
    flex-shrink: 0;
  }
  .issue-chip.pr :global(.issue-icon) {
    color: var(--success);
  }
  .issue-more {
    color: var(--text-dim);
    font-size: 11px;
  }

  /* Workbench is the row between the chat header and the composer.
     The transcript fills the remaining width on the left; the side
     panel (when open) docks to the right with its own resizer. The
     side panel lives inside this row (not full-window-height) so
     the composer below it can still span the full pane width. */
  .workbench {
    display: flex;
    flex: 1;
    min-height: 0;
  }
  .transcript-wrap {
    position: relative;
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .transcript {
    flex: 1;
    overflow-y: auto;
    min-height: 0;
  }
  .inner {
    max-width: 740px;
    margin: 0 auto;
    /* Extra right padding so the reading-positions gutter never
       crowds the message text on narrow viewports. */
    padding: 8px 56px 14px 40px;
  }

  .state {
    color: var(--text-dim);
    text-align: center;
    padding: 60px 24px;
    font-size: 13px;
  }
  .state.error { color: var(--danger); }
  .agent-loading {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    margin: 12px 0 8px;
    padding: 7px 10px;
    border-radius: var(--radius-sm);
    color: var(--text-muted);
    background: var(--bg-pill);
    font-size: 12px;
    line-height: 1.3;
  }
  .agent-loading.empty {
    margin: 48px auto;
    display: flex;
    width: fit-content;
  }
</style>
