<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import ActivityGraph from './components/ActivityGraph.svelte';
  import BuildTable from './components/BuildTable.svelte';
  import LogPanel from './components/LogPanel.svelte';
  import SummaryBar from './components/SummaryBar.svelte';
  import { openMonitorEvents } from './monitor-store';
  import { EMPTY_SNAPSHOT, type ConnectionStatus, type MonitorSnapshot } from './types';

  const SIDEBAR_KEY = 'nix-web-monitor.sidebar-width';
  const SIDEBAR_DEFAULT = 360;
  const SIDEBAR_MIN = 220;
  const SIDEBAR_MAX_FRACTION = 0.7;

  let snapshot = $state<MonitorSnapshot>(EMPTY_SNAPSHOT);
  let status = $state<ConnectionStatus>('connecting');
  let sidebarWidth = $state(loadSidebarWidth());
  let dragging = $state(false);
  let closeEvents: (() => void) | null = null;

  onMount(() => {
    closeEvents = openMonitorEvents(
      (nextSnapshot) => {
        snapshot = nextSnapshot;
      },
      (nextStatus) => {
        status = nextStatus;
      }
    );
  });

  onDestroy(() => {
    closeEvents?.();
  });

  function loadSidebarWidth(): number {
    if (typeof window === 'undefined') return SIDEBAR_DEFAULT;
    const stored = window.localStorage.getItem(SIDEBAR_KEY);
    if (stored === null) return SIDEBAR_DEFAULT;
    const parsed = Number(stored);
    return Number.isFinite(parsed) && parsed >= SIDEBAR_MIN ? parsed : SIDEBAR_DEFAULT;
  }

  function clampSidebarWidth(width: number): number {
    const max = Math.max(SIDEBAR_MIN, window.innerWidth * SIDEBAR_MAX_FRACTION);
    return Math.min(max, Math.max(SIDEBAR_MIN, width));
  }

  function onSplitterPointerDown(event: PointerEvent): void {
    dragging = true;
    event.preventDefault();
  }

  function onPointerMove(event: PointerEvent): void {
    if (!dragging) return;
    sidebarWidth = clampSidebarWidth(window.innerWidth - event.clientX);
  }

  function onPointerUp(): void {
    if (!dragging) return;
    dragging = false;
    window.localStorage.setItem(SIDEBAR_KEY, String(sidebarWidth));
  }

  function onSplitterKeydown(event: KeyboardEvent): void {
    const step = event.shiftKey ? 40 : 16;
    if (event.key === 'ArrowLeft') {
      sidebarWidth = clampSidebarWidth(sidebarWidth + step);
      event.preventDefault();
    } else if (event.key === 'ArrowRight') {
      sidebarWidth = clampSidebarWidth(sidebarWidth - step);
      event.preventDefault();
    } else {
      return;
    }
    window.localStorage.setItem(SIDEBAR_KEY, String(sidebarWidth));
  }
</script>

<svelte:window onpointermove={onPointerMove} onpointerup={onPointerUp} />

<main class:dragging>
  <SummaryBar {snapshot} {status} />

  <section class="workspace" style="--sidebar-width: {String(sidebarWidth)}px">
    <section class="main-pane">
      <LogPanel logs={snapshot.logs} />
    </section>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div
      class="splitter"
      role="separator"
      tabindex="0"
      aria-orientation="vertical"
      aria-label="Resize sidebar"
      aria-valuenow={Math.round(sidebarWidth)}
      onpointerdown={onSplitterPointerDown}
      onkeydown={onSplitterKeydown}
    ></div>
    <aside class="side-pane">
      <BuildTable builds={snapshot.builds} expected={snapshot.expected} />
      <ActivityGraph activities={snapshot.activities} builds={snapshot.builds} />
    </aside>
  </section>
</main>
