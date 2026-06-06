<script lang="ts">
  // Sidebar shell. Composes the top nav, search bar, two ThreadList
  // sections (active + archived), and the bottom settings bay. Owns
  // the flat cursor / keyboard nav model and the hover popover state;
  // everything else lives in its own component under components/sidebar/.
  //
  // Keyboard model (IntelliJ / nerdtree style):
  //   ⌘1                 → toggle focus (show + focus / focus / hide)
  //   j  / k / ↓ / ↑     → move cursor while focused
  //   Enter              → open chat under cursor
  //   e                  → archive chat under cursor
  //   Esc                → release focus (does not hide)
  //   g / G              → jump to top / bottom

  import { router } from '$lib/router';
  import { startNewChat } from '$lib/menu';
  import {
    sidebarActive,
    activateSidebar,
    deactivateSidebar,
    sidebarCollapsed
  } from '$lib/ui';
  import { tick } from 'svelte';
  import { DRAFT_STATUS } from '$lib/drafts';
  import { mergedThreadsList, type ServerThread } from '$lib/store';
  import * as api from '$lib/api';

  import SidebarTopNav from './sidebar/SidebarTopNav.svelte';
  import SidebarSearch from './sidebar/SidebarSearch.svelte';
  import SidebarSettings from './sidebar/SidebarSettings.svelte';
  import ThreadList from './sidebar/ThreadList.svelte';
  import IconCaretRight from '~icons/ph/caret-right';

  const ARCHIVED_STATUS = 'archived';
  const SHOW_MORE_LIMIT = 20;

  interface Props {
    activeServerId: string | null;
    activeThreadId: string | null;
  }

  let { activeServerId, activeThreadId }: Props = $props();

  let searching = $state(false);
  let search = $state('');
  let showAll = $state(false);
  // Archived chats are hidden by default — the "Archived (N)"
  // toggle below stays visible so they remain discoverable. Reset
  // per session: no persistence, since the closed state is the
  // intended default each time the app opens.
  let showArchived = $state(false);

  let active = $state(false);
  let cursor = $state(0);

  $effect(() => {
    const unsub = sidebarActive.subscribe((v) => {
      active = v;
      if (v) {
        // Sync cursor to the routed thread when we become active so
        // the first j/k feels predictable instead of jumping to the
        // top of the list.
        const idx = visible.findIndex(
          (t) => t.id === activeThreadId && t.server_id === activeServerId
        );
        cursor = idx >= 0 ? idx : 0;
        void scrollCursorIntoView();
      }
    });
    return unsub;
  });

  function previewCursor() {
    const t = visible[cursor];
    if (!t) return;
    if (t.id === activeThreadId && t.server_id === activeServerId) return;
    router.go(
      '/s/' + encodeURIComponent(t.server_id) + '/t/' + encodeURIComponent(t.id)
    );
  }

  let threads = $state<ServerThread[]>([]);
  const unsubThreads = mergedThreadsList.subscribe((v) => (threads = v));
  $effect(() => () => unsubThreads());

  let filtered = $derived(applyFilter(threads, search));
  // Archive forms a second section pinned below the active list. Each
  // section keeps its own updated_ms order; the server bumps
  // updated_ms on archive so the freshly archived row floats to the
  // top of the archived section.
  let split = $derived.by(() => {
    const activeList: ServerThread[] = [];
    const archivedList: ServerThread[] = [];
    for (const t of filtered) {
      if (t.status === ARCHIVED_STATUS) archivedList.push(t);
      else activeList.push(t);
    }
    return {
      active: activeList,
      archived: archivedList,
      all: [...activeList, ...archivedList]
    };
  });
  // `visible` is what the cursor can address. Skipping the archived
  // list when collapsed means j/k navigation can't get "lost" on
  // hidden rows and the cursor-bounds math stays trivial.
  let visible = $derived.by(() => {
    const list = showArchived ? split.all : split.active;
    return showAll ? list : list.slice(0, SHOW_MORE_LIMIT);
  });
  let visibleActive = $derived(visible.filter((t) => t.status !== ARCHIVED_STATUS));
  let visibleArchived = $derived(visible.filter((t) => t.status === ARCHIVED_STATUS));
  let moreCount = $derived.by(() => {
    const total = showArchived ? split.all.length : split.active.length;
    return total > SHOW_MORE_LIMIT ? total - SHOW_MORE_LIMIT : 0;
  });
  // Activity heatmap by ECDF percentile rank. Rank-based instead of
  // value/max because bimodal distributions (one chatty thread plus
  // many quiet ones) collapse linear/log scaling into "one bright row,
  // everyone else flat dim". Rank guarantees a smooth gradient across
  // the visible list regardless of distribution shape. Gamma > 1
  // pushes contrast to the top so hot spots actually pop while the
  // bottom half blends into the background tier. Ties share heat.
  const HEAT_GAMMA = 1.4;
  let heatById = $derived.by(() => {
    const map = new Map<string, number>();
    const n = visibleActive.length;
    if (n === 0) return map;
    if (n === 1) {
      map.set(visibleActive[0].id, 1);
      return map;
    }
    const sorted = [...visibleActive].sort(
      (a, b) => (a.message_count || 0) - (b.message_count || 0)
    );
    // Group ties and assign each tie group the midpoint rank of the
    // group so equal counts get equal heat.
    let i = 0;
    while (i < n) {
      let j = i;
      while (j < n && sorted[j].message_count === sorted[i].message_count) j++;
      const midRank = (i + j - 1) / 2;
      const norm = midRank / (n - 1);
      const heat = Math.pow(norm, HEAT_GAMMA);
      for (let k = i; k < j; k++) map.set(sorted[k].server_id + ':' + sorted[k].id, heat);
      i = j;
    }
    return map;
  });

  function applyFilter(list: ServerThread[], q: string): ServerThread[] {
    const needle = q.trim().toLowerCase();
    if (!needle) return list;
    return list.filter((t) => {
      const hay = [t.title, t.preview, t.user, t.host, t.repo ?? '', t.branch ?? '']
        .join(' ')
        .toLowerCase();
      return hay.includes(needle);
    });
  }

  async function archiveCursorRow() {
    const t = visible[cursor];
    if (!t) return;
    // Drafts only exist client-side, and archived rows are already
    // in the archive section; either way `e` is a no-op.
    if (t.status === DRAFT_STATUS) return;
    if (t.status === ARCHIVED_STATUS) return;
    try {
      await api.archiveThread(t.server_id, t.id);
      // The HTTP handler broadcasts the upsert before returning the
      // response, so by the time this resolves the ws delta has
      // landed and visibleActive has shrunk. Snap the cursor to the
      // row that took the archived one's place at the same visual
      // position, then preview it so the right pane moves on
      // instead of sitting on the just-archived chat.
      await tick();
      cursor = Math.max(0, Math.min(cursor, visibleActive.length - 1));
      void scrollCursorIntoView();
      previewCursor();
    } catch (err) {
      console.warn('room: archive failed', err);
    }
  }

  function go(thread: ServerThread, e: MouseEvent) {
    e.preventDefault();
    router.go(
      '/s/' + encodeURIComponent(thread.server_id) + '/t/' + encodeURIComponent(thread.id)
    );
  }

  function onNewChat(e?: MouseEvent) {
    if (e) e.preventDefault();
    startNewChat();
  }

  function startSearch() {
    searching = true;
    search = '';
    queueMicrotask(() => {
      const el = document.querySelector<HTMLInputElement>('.sidebar .search-input');
      el?.focus();
    });
  }

  function endSearch() {
    searching = false;
    search = '';
  }

  function toggleSearch() {
    if (searching) endSearch();
    else startSearch();
  }

  function submitSearchTop() {
    const top = visible[0];
    if (!top) return;
    router.go(
      '/s/' + encodeURIComponent(top.server_id) + '/t/' + encodeURIComponent(top.id)
    );
    endSearch();
    deactivateSidebar();
    queueMicrotask(() => {
      document.querySelector<HTMLTextAreaElement>('.composer textarea')?.focus();
    });
  }

  // Vim/nerdtree navigation. Active only while the sidebar owns the
  // keyboard (sidebarActive store). We swallow all keys while active
  // so the composer underneath can't double-handle them.
  function onKeydown(e: KeyboardEvent) {
    const target = e.target as HTMLElement | null;
    const inField =
      !!target &&
      (target.tagName === 'INPUT' ||
        target.tagName === 'TEXTAREA' ||
        target.isContentEditable);

    // `/` opens search from anywhere (when not already typing in a
    // field). Activates + reveals the sidebar if needed, then drops
    // the user into the filter input. Vim-style global search.
    if (
      e.key === '/' &&
      !e.metaKey &&
      !e.ctrlKey &&
      !e.altKey &&
      !inField
    ) {
      e.preventDefault();
      sidebarCollapsed.set(false);
      if (!active) activateSidebar();
      startSearch();
      return;
    }

    if (!active) return;
    // While searching, let the input field own the keyboard so the
    // user can type filter text. The search input handles its own
    // Esc / Enter through SidebarSearch.
    const inSearchField =
      !!target && target.classList?.contains('search-input');
    if (inSearchField) return;

    if (e.key === 'Escape') {
      // Drop into the transcript vim layer, not the composer.
      // stopImmediatePropagation keeps ThreadDetail's *sibling*
      // window-level Esc handler from observing the deactivation we
      // just performed and greedily focusing the composer textarea
      // on the same keystroke. (stopPropagation alone wouldn't help
      // because both handlers are attached to `window` directly.)
      e.preventDefault();
      e.stopImmediatePropagation();
      deactivateSidebar();
      return;
    }
    if (visible.length === 0) return;

    if (e.key === 'j' || e.key === 'ArrowDown') {
      e.preventDefault();
      cursor = Math.min(visible.length - 1, cursor + 1);
      void scrollCursorIntoView();
      previewCursor();
      return;
    }
    if (e.key === 'k' || e.key === 'ArrowUp') {
      e.preventDefault();
      cursor = Math.max(0, cursor - 1);
      void scrollCursorIntoView();
      previewCursor();
      return;
    }
    if (e.key === 'g') {
      e.preventDefault();
      cursor = 0;
      void scrollCursorIntoView();
      previewCursor();
      return;
    }
    if (e.key === 'G') {
      e.preventDefault();
      cursor = visible.length - 1;
      void scrollCursorIntoView();
      previewCursor();
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      deactivateSidebar();
      queueMicrotask(() => {
        document.querySelector<HTMLTextAreaElement>('.composer textarea')?.focus();
      });
      return;
    }
    if (e.key === 'e' && !e.metaKey && !e.ctrlKey && !e.altKey) {
      e.preventDefault();
      archiveCursorRow();
      return;
    }
  }

  async function scrollCursorIntoView() {
    await tick();
    const el = document.querySelector<HTMLElement>(
      `.sidebar .row[data-cursor-index="${cursor}"]`
    );
    el?.scrollIntoView({ block: 'nearest' });
  }

  $effect(() => {
    window.addEventListener('keydown', onKeydown);
    return () => window.removeEventListener('keydown', onKeydown);
  });
</script>

<aside class="sidebar" class:active>
  <SidebarTopNav {searching} {onNewChat} onToggleSearch={toggleSearch} />

  {#if searching}
    <SidebarSearch
      value={search}
      onChange={(next) => (search = next)}
      onSubmit={submitSearchTop}
      onCancel={endSearch}
    />
  {/if}

  <div class="section-label">Chats</div>

  <nav class="threads">
    {#if filtered.length === 0}
      <div class="empty">
        {threads.length === 0 ? 'No chats yet.' : 'No chats match.'}
      </div>
    {:else}
      <ThreadList
        label="Chats"
        threads={visibleActive}
        cursorOffset={0}
        activeCursor={active ? cursor : null}
        routedKey={activeServerId && activeThreadId ? activeServerId + ':' + activeThreadId : null}
        {heatById}
        onOpen={go}
      />

      {#if split.archived.length > 0}
        <button
          class="archive-toggle"
          aria-expanded={showArchived}
          onclick={() => (showArchived = !showArchived)}
        >
          <IconCaretRight class="archive-caret {showArchived ? 'open' : ''}" width={11} height={11} />
          <span class="archive-label">Archived</span>
          <span class="archive-count">{split.archived.length}</span>
        </button>
        {#if showArchived}
          <ThreadList
            label="Archived"
            threads={visibleArchived}
            cursorOffset={visibleActive.length}
            activeCursor={active ? cursor : null}
            routedKey={activeServerId && activeThreadId ? activeServerId + ':' + activeThreadId : null}
            onOpen={go}
          />
        {/if}
      {/if}

      {#if moreCount > 0 || showAll}
        <button class="show-more" onclick={() => (showAll = !showAll)}>
          {showAll ? 'Show less' : `Show ${moreCount} more`}
        </button>
      {/if}
    {/if}
  </nav>

  <SidebarSettings />
</aside>

<style>
  .sidebar {
    display: grid;
    /* topnav · section-label · threads (flex) · settings-bay */
    grid-template-rows: auto auto 1fr auto;
    height: 100%;
    min-height: 0;
    /* Transparent so the window-level NSVisualEffectView vibrancy
       shows through. The sidebar appears as native frosted glass. */
    background: transparent;
    color: var(--text);
    min-width: 0;
    font-feature-settings: 'cv02', 'cv03', 'cv04', 'cv11';
    padding-top: 35px;
  }

  .section-label {
    padding: 14px 22px 5px;
    color: var(--text-dim);
    font-size: 12px;
    font-weight: 400;
  }

  .threads {
    min-height: 0;
    overflow-y: auto;
    /* Per CSS spec, declaring only overflow-y promotes overflow-x to
       `auto` as well, so any transient horizontal ink overflow
       (idle-peer zzz badge sliding in, avatar ring pulse, glyph font
       fallback width jitter) flashes a horizontal scrollbar. The row
       list legitimately never scrolls sideways — make that explicit. */
    overflow-x: clip;
    padding: 0 12px 8px;
  }
  .empty {
    color: var(--text-dim);
    font-size: 12px;
    padding: 18px 12px;
    text-align: center;
  }

  .show-more {
    display: block;
    width: 100%;
    text-align: left;
    padding: 6px 10px;
    color: var(--text-dim);
    font-size: 12px;
    border-radius: 6px;
    cursor: pointer;
  }
  .show-more:hover {
    color: var(--text-muted);
    background: var(--bg-hover);
  }

  /* Mirrors .section-label spacing so the row reads as the section
     header it is, with the count pinned right and a disclosure caret
     on the left. Phosphor's caret-right rotates on toggle. */
  .archive-toggle {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 16px 8px 4px;
    color: var(--text-dim);
    font-size: 12px;
    font-weight: 400;
    text-align: left;
    cursor: pointer;
  }
  .archive-toggle:hover {
    color: var(--text-muted);
  }
  .archive-toggle :global(.archive-caret) {
    flex-shrink: 0;
    transition: transform 0.12s ease;
  }
  .archive-toggle :global(.archive-caret.open) {
    transform: rotate(90deg);
  }
  .archive-count {
    margin-left: auto;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
</style>
