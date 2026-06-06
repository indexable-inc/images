<script lang="ts">
  // Cmd-F find bar for the active thread.
  //
  // Matches are painted with the CSS Custom Highlight API so the
  // overlay sits on top of whatever the message renderers (markdown,
  // shiki, etc.) produced — we never mutate transcript DOM.
  //
  // Two correctness rules drive the shape below:
  //
  //   1. Range objects must not outlive a single rebuild. MarkdownBody
  //      renders with {@html ...}; any assistant token bump replaces
  //      its text nodes wholesale. A cached Range from a previous
  //      rebuild references detached nodes the moment that happens,
  //      and WebKit can keep painting the old overlay for a frame
  //      or two before invalidation catches up. So Ranges live in a
  //      local of `rebuild` and never escape.
  //
  //   2. The registry is cleared eagerly on every input change. The
  //      throttled rebuild runs ~one frame later; without an eager
  //      clear the stale overlay would linger for the whole throttle
  //      window any time the user edits the query.

  import { onDestroy, tick, untrack } from 'svelte';

  interface Props {
    open: boolean;
    scroller: HTMLElement | undefined;
    /** Bumped by the parent each time ⌘F is pressed so a repeat
     *  press while the bar is already open refocuses + selects the
     *  input, matching browser-find behavior. */
    focusBump: number;
    onClose: () => void;
  }

  let { open, scroller, focusBump, onClose }: Props = $props();

  let query = $state('');
  let activeIdx = $state(0);
  let matchCount = $state(0);
  let inputEl: HTMLInputElement | undefined = $state();

  const NAME = 'chat-find';
  const ACTIVE = 'chat-find-active';
  // Bound the work per rebuild so a one-character query on a huge
  // transcript stays interactive. Far past what a human can step
  // through with ⏎ anyway.
  const MAX_MATCHES = 1000;
  // Trailing throttle: at most one rebuild per window, no matter
  // how fast the user types. Roughly one frame at 60 Hz so typing
  // feels live without re-walking the DOM on every keystroke.
  const THROTTLE_MS = 80;

  const canHighlight = typeof CSS !== 'undefined' && 'highlights' in CSS;

  // Only the active match's host element is cached so step() can
  // scrollIntoView without re-running the walker. Set during paint,
  // cleared whenever the registry is cleared.
  let activeHost: HTMLElement | null = null;

  let rebuildTimer: ReturnType<typeof setTimeout> | null = null;

  function clearRegistry() {
    if (!canHighlight) return;
    CSS.highlights.delete(NAME);
    CSS.highlights.delete(ACTIVE);
    activeHost = null;
  }

  function search(root: HTMLElement, needle: string): Range[] {
    const lower = needle.toLowerCase();
    const out: Range[] = [];
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    let node: Node | null;
    outer: while ((node = walker.nextNode())) {
      const text = node.nodeValue;
      if (!text) continue;
      const tag = node.parentElement?.tagName;
      if (tag === 'SCRIPT' || tag === 'STYLE') continue;
      const hay = text.toLowerCase();
      let from = 0;
      for (;;) {
        const i = hay.indexOf(lower, from);
        if (i === -1) break;
        const r = new Range();
        r.setStart(node, i);
        r.setEnd(node, i + needle.length);
        out.push(r);
        if (out.length >= MAX_MATCHES) break outer;
        from = i + needle.length;
      }
    }
    return out;
  }

  function paint(ranges: Range[], idx: number) {
    if (!canHighlight) return;
    clearRegistry();
    if (ranges.length === 0) return;
    const others: Range[] = [];
    const active: Range[] = [];
    for (let i = 0; i < ranges.length; i++) {
      (i === idx ? active : others).push(ranges[i]!);
    }
    try {
      if (others.length > 0) CSS.highlights.set(NAME, new Highlight(...others));
      if (active.length > 0) CSS.highlights.set(ACTIVE, new Highlight(...active));
    } catch {
      clearRegistry();
      return;
    }
    const start = active[0]?.startContainer;
    activeHost =
      start?.nodeType === Node.ELEMENT_NODE
        ? (start as HTMLElement)
        : (start?.parentElement ?? null);
  }

  function rebuild() {
    if (!open || !scroller || !query) {
      matchCount = 0;
      activeIdx = 0;
      clearRegistry();
      return;
    }
    const ranges = search(scroller, query);
    matchCount = ranges.length;
    if (activeIdx >= matchCount) activeIdx = 0;
    paint(ranges, activeIdx);
    void tick().then(() => scrollActive(false));
  }

  function scheduleRebuild() {
    // Trailing throttle. Once a rebuild is scheduled, don't reset
    // the timer — otherwise a continuous stream of input events
    // would keep pushing the deadline forward and the rebuild would
    // never actually fire.
    if (rebuildTimer) return;
    rebuildTimer = setTimeout(() => {
      rebuildTimer = null;
      rebuild();
    }, THROTTLE_MS);
  }

  function scrollActive(smooth: boolean) {
    const host = activeHost;
    if (!host || !scroller) return;
    const h = host.getBoundingClientRect();
    const s = scroller.getBoundingClientRect();
    if (h.top < s.top + 40 || h.bottom > s.bottom - 40) {
      host.scrollIntoView({ block: 'center', behavior: smooth ? 'smooth' : 'auto' });
    }
  }

  function step(delta: 1 | -1) {
    if (matchCount === 0) return;
    activeIdx = (activeIdx + delta + matchCount) % matchCount;
    // Cheap re-search — the throttle window is plenty short to keep
    // the DOM stable for one frame, and re-running here keeps step()
    // honest against any re-render that happened since the last
    // rebuild instead of trusting stale Range refs.
    if (scroller && query) {
      const ranges = search(scroller, query);
      matchCount = ranges.length;
      if (activeIdx >= matchCount) activeIdx = 0;
      paint(ranges, activeIdx);
    }
    scrollActive(true);
  }

  // Single effect for the whole rebuild lifecycle. Eagerly clears
  // the registry on every input/state change so stale paint can't
  // linger across the throttle window, then schedules the actual
  // search. Closing the bar drops the query so reopening starts
  // empty.
  $effect(() => {
    void query;
    void scroller;
    void open;
    untrack(() => {
      clearRegistry();
      if (!open) {
        matchCount = 0;
        activeIdx = 0;
        query = '';
        if (rebuildTimer) {
          clearTimeout(rebuildTimer);
          rebuildTimer = null;
        }
        return;
      }
      scheduleRebuild();
    });
  });

  $effect(() => {
    void focusBump;
    if (!open) return;
    void tick().then(() => {
      inputEl?.focus();
      inputEl?.select();
    });
  });

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      step(e.shiftKey ? -1 : 1);
    } else if ((e.metaKey || e.ctrlKey) && (e.key === 'g' || e.key === 'G')) {
      // ⌘G / ⇧⌘G — Apple's "Find Next / Previous" convention.
      e.preventDefault();
      step(e.shiftKey ? -1 : 1);
    }
  }

  onDestroy(() => {
    if (rebuildTimer) clearTimeout(rebuildTimer);
    clearRegistry();
  });
</script>

{#if open}
  <div class="find-bar" role="search">
    <input
      bind:this={inputEl}
      bind:value={query}
      onkeydown={onKey}
      class="find-input"
      placeholder="Find in chat"
      spellcheck="false"
      autocomplete="off"
      autocapitalize="off"
      autocorrect="off"
      aria-label="Find in chat"
    />
    <span class="count" aria-live="polite">
      {#if query.length === 0}
        &nbsp;
      {:else if matchCount === 0}
        No matches
      {:else}
        {activeIdx + 1} / {matchCount}
      {/if}
    </span>
    <button
      type="button"
      class="nav"
      title="Previous match (⇧⏎)"
      aria-label="Previous match"
      disabled={matchCount === 0}
      onclick={() => step(-1)}
    >↑</button>
    <button
      type="button"
      class="nav"
      title="Next match (⏎)"
      aria-label="Next match"
      disabled={matchCount === 0}
      onclick={() => step(1)}
    >↓</button>
    <button
      type="button"
      class="close"
      title="Close (Esc)"
      aria-label="Close find"
      onclick={onClose}
    >✕</button>
  </div>
{/if}

<style>
  .find-bar {
    position: absolute;
    top: 8px;
    right: 16px;
    z-index: 20;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 5px 6px 5px 10px;
    background: var(--bg-elev);
    border-radius: var(--radius);
    box-shadow: var(--shadow-popover);
    font-size: 12.5px;
  }
  .find-input {
    border: none;
    outline: none;
    background: transparent;
    color: var(--text-strong);
    width: 200px;
    padding: 2px 0;
  }
  .find-input::placeholder { color: var(--text-dim); }
  .count {
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
    font-size: 11.5px;
    min-width: 56px;
    text-align: right;
  }
  .nav, .close {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: var(--radius-sm);
    color: var(--text-muted);
    font-size: 12px;
    line-height: 1;
  }
  .nav:hover:not(:disabled), .close:hover {
    background: var(--bg-hover);
    color: var(--text-strong);
  }
  .nav:disabled {
    opacity: 0.4;
  }
  .close {
    margin-left: 2px;
  }
</style>
