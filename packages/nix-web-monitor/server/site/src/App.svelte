<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import ActivityGraph from '$components/ActivityGraph.svelte';
  import BuildTable from '$components/BuildTable.svelte';
  import ErrorPanel from '$components/ErrorPanel.svelte';
  import LogPanel from '$components/LogPanel.svelte';
  import SummaryBar from '$components/SummaryBar.svelte';
  import Splitter from '$lib/Splitter.svelte';
  import { openMonitorEvents } from '$lib/monitor-store';
  import { EMPTY_SNAPSHOT, type ConnectionStatus, type MonitorSnapshot } from '$lib/types';

  const SIDEBAR_KEY = 'nix-web-monitor.sidebar-width';
  const SIDEBAR_DEFAULT = 360;
  const SIDEBAR_MIN = 220;
  const SIDEBAR_MAX_FRACTION = 0.7;

  const BUILDS_KEY = 'nix-web-monitor.builds-fraction';
  const BUILDS_DEFAULT = 0.45;
  const BUILDS_MIN_FRACTION = 0.15;
  const BUILDS_MAX_FRACTION = 0.85;

  let snapshot = $state<MonitorSnapshot>(EMPTY_SNAPSHOT);
  let status = $state<ConnectionStatus>('connecting');
  let sidebarWidth = $state(loadNumber(SIDEBAR_KEY, SIDEBAR_DEFAULT, SIDEBAR_MIN));
  let buildsFraction = $state(
    loadNumber(BUILDS_KEY, BUILDS_DEFAULT, BUILDS_MIN_FRACTION, BUILDS_MAX_FRACTION)
  );
  let draggingAxis = $state<'horizontal' | 'vertical' | null>(null);
  let sidePane = $state<HTMLElement | null>(null);
  /// When set, the log panel filters to entries whose activityId matches this
  /// build's activity. Clicking the same build again or hitting the clear
  /// chip in the log panel resets it.
  let selectedActivityId = $state<number | null>(null);
  /// Log panel instance, used to drive its filter from the errors panel. Typed
  /// to the imperative surface we call so the binding stays checked rather than
  /// collapsing to `any`.
  type LogPanelApi = { inspect: (text: string) => void };
  let logPanel = $state<LogPanelApi | null>(null);
  /// Number of errors the operator has dismissed; the panel reappears only when
  /// a newer error pushes the count past this watermark.
  let errorsDismissed = $state(0);
  let closeEvents: (() => void) | null = null;

  const showErrors = $derived(snapshot.errors.length > errorsDismissed);

  function dismissErrors(): void {
    errorsDismissed = snapshot.errors.length;
  }

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

  function loadNumber(key: string, fallback: number, min: number, max?: number): number {
    if (typeof window === 'undefined') return fallback;
    const stored = window.localStorage.getItem(key);
    if (stored === null) return fallback;
    const parsed = Number(stored);
    if (!Number.isFinite(parsed)) return fallback;
    if (parsed < min) return fallback;
    if (max !== undefined && parsed > max) return fallback;
    return parsed;
  }

  function persist(key: string, value: number): void {
    window.localStorage.setItem(key, String(value));
  }

  function clampSidebarWidth(width: number): number {
    const max = Math.max(SIDEBAR_MIN, window.innerWidth * SIDEBAR_MAX_FRACTION);
    return Math.min(max, Math.max(SIDEBAR_MIN, width));
  }

  function clampBuildsFraction(fraction: number): number {
    return Math.min(BUILDS_MAX_FRACTION, Math.max(BUILDS_MIN_FRACTION, fraction));
  }

  function onPointerMove(event: PointerEvent): void {
    if (draggingAxis === 'horizontal') {
      sidebarWidth = clampSidebarWidth(window.innerWidth - event.clientX);
    } else if (draggingAxis === 'vertical' && sidePane !== null) {
      const rect = sidePane.getBoundingClientRect();
      buildsFraction = clampBuildsFraction((event.clientY - rect.top) / rect.height);
    }
  }

  function onPointerUp(): void {
    if (draggingAxis === 'horizontal') persist(SIDEBAR_KEY, sidebarWidth);
    else if (draggingAxis === 'vertical') persist(BUILDS_KEY, buildsFraction);
    draggingAxis = null;
  }

  function startHorizontal(event: PointerEvent): void {
    draggingAxis = 'horizontal';
    event.preventDefault();
  }

  function startVertical(event: PointerEvent): void {
    draggingAxis = 'vertical';
    event.preventDefault();
  }

  function sidebarKeydown(event: KeyboardEvent): void {
    const step = event.shiftKey ? 40 : 16;
    if (event.key === 'ArrowLeft') {
      sidebarWidth = clampSidebarWidth(sidebarWidth + step);
    } else if (event.key === 'ArrowRight') {
      sidebarWidth = clampSidebarWidth(sidebarWidth - step);
    } else {
      return;
    }
    event.preventDefault();
    persist(SIDEBAR_KEY, sidebarWidth);
  }

  function buildsKeydown(event: KeyboardEvent): void {
    const step = event.shiftKey ? 0.08 : 0.03;
    if (event.key === 'ArrowUp') {
      buildsFraction = clampBuildsFraction(buildsFraction - step);
    } else if (event.key === 'ArrowDown') {
      buildsFraction = clampBuildsFraction(buildsFraction + step);
    } else {
      return;
    }
    event.preventDefault();
    persist(BUILDS_KEY, buildsFraction);
  }
</script>

<svelte:window onpointermove={onPointerMove} onpointerup={onPointerUp} />

<main
  class:dragging-h={draggingAxis === 'horizontal'}
  class:dragging-v={draggingAxis === 'vertical'}
>
  <div class="topbar">
    <SummaryBar {snapshot} {status} />
    {#if showErrors}
      <ErrorPanel
        errors={snapshot.errors}
        onclose={dismissErrors}
        oninspect={(text: string) => logPanel?.inspect(text)}
      />
    {/if}
  </div>

  <section class="workspace" style="--sidebar-width: {String(sidebarWidth)}px">
    <section class="main-pane">
      <LogPanel
        bind:this={logPanel}
        logs={snapshot.logs}
        {selectedActivityId}
        onclearselection={() => (selectedActivityId = null)}
      />
    </section>
    <Splitter
      orientation="vertical"
      label="Resize sidebar"
      valueNow={Math.round(sidebarWidth)}
      onstart={startHorizontal}
      onkeydown={sidebarKeydown}
    />
    <aside
      class="side-pane"
      bind:this={sidePane}
      style="--builds-fraction: {String(buildsFraction)}"
    >
      <BuildTable
        builds={snapshot.builds}
        dependencies={snapshot.dependencies}
        expected={snapshot.expected}
        {selectedActivityId}
        onselect={(id: number | null) => {
          selectedActivityId = id;
        }}
      />
      <Splitter
        orientation="horizontal"
        label="Resize builds panel"
        valueNow={Math.round(buildsFraction * 100)}
        onstart={startVertical}
        onkeydown={buildsKeydown}
      />
      <ActivityGraph activities={snapshot.activities} builds={snapshot.builds} />
    </aside>
  </section>
</main>
