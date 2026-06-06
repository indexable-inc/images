<script lang="ts">
  import { get } from 'svelte/store';
  import { router } from '$lib/router';
  import { bindMenu, setTrafficLightsVisible, hapticFeedback, startNewChat } from '$lib/menu';
  import { bindDeepLinks } from '$lib/deepLink';
  import { nextThread, previousThread } from '$lib/commands';
  import {
    sidebarCollapsed,
    sidebarWidth,
    setSidebarWidth,
    settingsOpen,
    closeSettings,
    paletteOpen,
    togglePalette,
    closePalette,
    toggleSidebarFocus,
    deactivateSidebar,
    SIDEBAR_COLLAPSE_THRESHOLD,
    SIDEBAR_MIN_WIDTH,
    SIDEBAR_MAX_WIDTH
  } from '$lib/ui';
  import Sidebar from '$components/Sidebar.svelte';
  import CommandPalette from '$components/CommandPalette.svelte';
  import IdentityModal from '$components/IdentityModal.svelte';
  import NewChatModal from '$components/NewChatModal.svelte';
  import SettingsModal from '$components/SettingsModal.svelte';
  // import FpsOverlay from '$components/FpsOverlay.svelte';
  import StatusBar from '$components/StatusBar.svelte';
  import ThreadDetail from '$routes/ThreadDetail.svelte';
  import { roomFor } from '$lib/store';
  import { getRoomServer } from '$lib/backend';
  import type { Route } from '$lib/router';
  import type { Thread } from '$lib/types';

  let route = $state<Route>({ name: 'threads' });
  const unsub = router.subscribe((r) => (route = r));
  $effect(() => () => unsub());
  let routeServerExists = $derived(
    route.name !== 'thread' || getRoomServer(route.serverId) !== undefined
  );

  // Thread cache mirrored locally so the status bar can derive the
  // current thread without each consumer re-subscribing. The bar
  // hides for drafts (no server-resolved thread to set a goal on)
  // and for non-thread routes.
  let threadsMap = $state(new Map<string, Thread>());
  $effect(() => {
    if (route.name !== 'thread' || !routeServerExists) {
      threadsMap = new Map();
      return;
    }
    const store = roomFor(route.serverId);
    const unsubThreads = store.threads.subscribe((m) => (threadsMap = m));
    return unsubThreads;
  });

  let currentThread = $derived(
    route.name === 'thread' && routeServerExists
      ? threadsMap.get(route.threadId) ?? null
      : null
  );

  // Skip the welcome pane: boot straight into a fresh chat. The
  // sidebar still navigates between existing threads; this only
  // fires on the bare `/` route so refreshing a thread URL keeps
  // you on that thread.
  $effect(() => {
    if (route.name === 'threads') {
      startNewChat();
    }
  });

  let collapsed = $state(false);
  const unsubCollapsed = sidebarCollapsed.subscribe((v) => (collapsed = v));
  $effect(() => () => unsubCollapsed());

  let width = $state(264);
  const unsubWidth = sidebarWidth.subscribe((v) => (width = v));
  $effect(() => () => unsubWidth());

  // Drag-to-resize. While dragging we suspend the grid transition so
  // the handle tracks the cursor 1:1, and listen on window so the
  // drag survives mouse leaving the handle element.
  let dragging = $state(false);

  function onResizerDown(e: MouseEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    if (collapsed) sidebarCollapsed.set(false);
    dragging = true;
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';

    // Track which side of each boundary we were on last frame so we
    // can buzz exactly once per crossing — both directions feel snappy
    // like Finder's column resize.
    let pastCollapse = width < SIDEBAR_COLLAPSE_THRESHOLD;
    let atMin = width <= SIDEBAR_MIN_WIDTH;
    let atMax = width >= SIDEBAR_MAX_WIDTH;

    const onMove = (m: MouseEvent) => {
      const next = m.clientX;
      const nowPast = next < SIDEBAR_COLLAPSE_THRESHOLD;
      if (nowPast !== pastCollapse) {
        void hapticFeedback('alignment');
        pastCollapse = nowPast;
      }

      const nowAtMin = next <= SIDEBAR_MIN_WIDTH && next >= SIDEBAR_COLLAPSE_THRESHOLD;
      const nowAtMax = next >= SIDEBAR_MAX_WIDTH;
      if (nowAtMin && !atMin) {
        void hapticFeedback('alignment');
      }
      if (nowAtMax && !atMax) {
        void hapticFeedback('alignment');
      }
      atMin = nowAtMin;
      atMax = nowAtMax;

      if (nowPast) {
        setSidebarWidth(SIDEBAR_MIN_WIDTH);
      } else {
        setSidebarWidth(next);
      }
    };
    const onUp = (u: MouseEvent) => {
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
      dragging = false;
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      if (u.clientX < SIDEBAR_COLLAPSE_THRESHOLD) {
        sidebarCollapsed.set(true);
      }
    };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  }

  // When the sidebar is collapsed we still want the traffic lights to
  // appear if the user mouses into the top-left so they can close /
  // minimise / zoom. Track a derived "hovering the reveal zone" flag
  // and reapply visibility every time it (or the collapsed state)
  // changes. A short hide delay keeps the buttons usable: as you move
  // the cursor from the empty band toward the actual buttons you
  // briefly leave the reveal zone, and without the delay the buttons
  // would vanish under your cursor.
  let revealLights = $state(false);
  let hideTimer: ReturnType<typeof setTimeout> | undefined;
  const REVEAL_W = 120;
  const REVEAL_H = 38;
  const HIDE_DELAY_MS = 450;

  function inRevealZone(x: number, y: number): boolean {
    return x >= 0 && x <= REVEAL_W && y >= 0 && y <= REVEAL_H;
  }

  function onMouseMove(e: MouseEvent) {
    if (!collapsed) return;
    const within = inRevealZone(e.clientX, e.clientY);
    if (within) {
      if (hideTimer) {
        clearTimeout(hideTimer);
        hideTimer = undefined;
      }
      if (!revealLights) revealLights = true;
    } else if (revealLights && !hideTimer) {
      hideTimer = setTimeout(() => {
        revealLights = false;
        hideTimer = undefined;
      }, HIDE_DELAY_MS);
    }
  }

  $effect(() => {
    window.addEventListener('mousemove', onMouseMove);
    return () => {
      window.removeEventListener('mousemove', onMouseMove);
      if (hideTimer) clearTimeout(hideTimer);
    };
  });

  let lightsVisible = $derived(!collapsed || revealLights);
  $effect(() => {
    void setTrafficLightsVisible(lightsVisible);
  });

  // Global keyboard:
  //   ⌘K / Ctrl+K  → toggle palette (always — works in browser dev too)
  //   Esc          → close palette > close settings, but only when no
  //                  text field owns the keystroke
  $effect(() => {
    function onKey(e: KeyboardEvent) {
      const target = e.target as HTMLElement | null;
      const inField =
        !!target &&
        (target.tagName === 'INPUT' ||
          target.tagName === 'TEXTAREA' ||
          target.isContentEditable);

      // ⌘K works even when typing — that's the expected affordance.
      if ((e.metaKey || e.ctrlKey) && (e.key === 'k' || e.key === 'K')) {
        e.preventDefault();
        togglePalette();
        return;
      }

      // ⌘1 / ⌘S — IntelliJ-style sidebar focus toggle.
      //   hidden            → show + focus
      //   shown + focused   → hide + drop focus
      //   shown + unfocused → focus only
      // ⌘S aliases ⌘1 (Save has no meaning in a chat surface and ⌘S
      // is closer to muscle memory for sidebar reveal on some
      // keyboards).
      if (
        (e.metaKey || e.ctrlKey) &&
        !e.shiftKey &&
        !e.altKey &&
        (e.code === 'Digit1' || e.code === 'KeyS')
      ) {
        e.preventDefault();
        toggleSidebarFocus();
        return;
      }

      // ⇧⌘] / ⇧⌘[ secondary binding for next/previous chat. The ⌘]
      // / ⌘[ primary binding is handled by the Tauri menu accelerator;
      // catching the Shift variant here covers users who reach for the
      // tab-style shortcut Safari/Chrome use. Use e.code so a shifted
      // ] (which keyboards report as `}`) still resolves correctly.
      if ((e.metaKey || e.ctrlKey) && e.shiftKey) {
        if (e.code === 'BracketRight') {
          e.preventDefault();
          nextThread();
          return;
        }
        if (e.code === 'BracketLeft') {
          e.preventDefault();
          previousThread();
          return;
        }
      }

      if (e.key === 'Escape') {
        if (get(paletteOpen)) {
          closePalette();
          e.preventDefault();
          return;
        }
        if (inField) {
          // Blur the focused input so viewer-mode keys (u/d/g/G,
          // j/k after ⌘1, etc.) reach their handlers. The component
          // owning the field may also do its own Esc work — this is
          // a safety net that fires regardless. Don't preventDefault
          // so the component still sees the keydown.
          target?.blur();
          return;
        }
        if (get(settingsOpen)) {
          closeSettings();
          e.preventDefault();
        }
      }
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  $effect(() => {
    let cancelled = false;
    let cleanup: (() => void) | undefined;
    bindMenu().then((fn) => {
      if (cancelled) fn();
      else cleanup = fn;
    });
    return () => {
      cancelled = true;
      cleanup?.();
    };
  });

  $effect(() => {
    let cancelled = false;
    let cleanup: (() => void) | undefined;
    bindDeepLinks().then((fn) => {
      if (cancelled) fn();
      else cleanup = fn;
    });
    return () => {
      cancelled = true;
      cleanup?.();
    };
  });
</script>

<div class="window">
  <div
    class="shell"
    class:sidebar-collapsed={collapsed}
    class:dragging
    style="--sidebar-w: {collapsed ? 0 : width}px"
  >
    <Sidebar
      activeServerId={route.name === 'thread' && routeServerExists ? route.serverId : null}
      activeThreadId={route.name === 'thread' && routeServerExists ? route.threadId : null}
    />

    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div
      class="sidebar-resizer"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize sidebar"
      onmousedown={onResizerDown}
      ondblclick={() => sidebarCollapsed.set(!collapsed)}
    ></div>

    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <!-- onmousedown only: a real user click in the pane drops sidebar
         focus. We deliberately do NOT listen to `focusin`, because the
         Composer auto-focuses its textarea on every thread switch — and
         preview navigation switches threads on every j/k. Listening to
         focusin would kill the preview mode after one keystroke. -->
    <main class="pane" onmousedown={() => deactivateSidebar()}>
      {#if route.name === 'thread' && routeServerExists}
        <ThreadDetail serverId={route.serverId} threadId={route.threadId} />
      {:else if route.name === 'not-found'}
        <div class="nf">Not found.</div>
      {/if}
    </main>
  </div>

  <!-- IntelliJ-style status bar spanning the entire window bottom
       (under sidebar + pane both). Drawer panels (full goal editor /
       plan list) open as bottom-docked tool-window strips above the
       bar, pushing the shell up — same model IntelliJ uses for
       Terminal / Run / Problems. Always rendered: identity + connection
       are universal. StatusBar itself hides the thread-specific
       segments (goal, plan, work, model, messages) when there's no
       server-resolved thread, so drafts and the not-found route still
       get the "me" avatar and the connection dot without lighting up a
       Set-goal pill that has nothing to pin to. -->
  <StatusBar
    serverId={route.name === 'thread' && routeServerExists ? route.serverId : null}
    thread={currentThread}
  />
</div>

<CommandPalette />
<IdentityModal />
<NewChatModal />
<SettingsModal />
<!-- <FpsOverlay /> -->


<style>
  /* IntelliJ-style: window is a column. Shell (sidebar + pane)
     takes whatever vertical space is left after the status bar
     claims its 26px at the bottom. The native macOS window radius
     lives here so the corners round below the status bar too. */
  .window {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
    border-radius: 10px;
  }
  .shell {
    position: relative;
    display: grid;
    grid-template-columns: var(--sidebar-w) 0 1fr;
    flex: 1;
    min-height: 0;
    overflow: hidden;
    transition: grid-template-columns 0.18s ease;
  }
  /* Hot drag: no transition so the handle tracks the cursor 1:1. */
  .shell.dragging {
    transition: none;
  }
  .shell.sidebar-collapsed {
    grid-template-columns: 0 0 1fr;
  }
  .shell.sidebar-collapsed :global(.sidebar) {
    visibility: hidden;
  }

  /* The resizer sits as its own (zero-width) grid column so the
     pane stays adjacent to the sidebar; we widen the hit area with
     negative margins and absolute positioning. */
  .sidebar-resizer {
    position: relative;
    width: 0;
    cursor: col-resize;
    z-index: 5;
  }
  .sidebar-resizer::before {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: -3px;
    width: 7px;
  }
  /* Resizer line only appears on hover / drag — at rest the sidebar
     and pane share the same backdrop without any visible divider. */
  .sidebar-resizer::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    width: 1px;
    background: transparent;
    transition: background 0.15s ease, width 0.15s ease;
  }
  .sidebar-resizer:hover::after,
  .shell.dragging .sidebar-resizer::after {
    background: var(--border-hi);
    width: 2px;
  }
  .shell.sidebar-collapsed .sidebar-resizer {
    pointer-events: none;
    opacity: 0;
  }
  /* Solid main pane (Mail / Messages two-tone). The sidebar stays
     transparent over the window's NSVisualEffectView; the pane sits
     on top of it as opaque material. No border-left — the seam
     between the two materials is enough on its own. */
  .pane {
    background: var(--bg-pane);
    min-width: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  /* When the sidebar is collapsed the traffic lights are hidden too
     (Arc-style — they only appear on hover), so the main pane can
     run full-bleed to the top of the window. If the user hovers
     the reveal zone the buttons briefly overlay the thread header,
     which is acceptable for the extra real estate. */
  .nf {
    padding: 80px 24px;
    text-align: center;
    color: var(--text-dim);
  }
  @media (max-width: 720px) {
    .shell {
      grid-template-columns: 1fr;
    }
    .pane {
      border-left: none;
      border-top: 1px solid var(--border);
    }
  }
</style>
