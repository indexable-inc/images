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

  /// Width of the live-activity sidebar that sits to the right of the build DAG.
  const SIDEBAR_KEY = 'nix-web-monitor.sidebar-width';
  const SIDEBAR_DEFAULT = 380;
  const SIDEBAR_MIN = 220;
  const SIDEBAR_MAX_FRACTION = 0.6;

  /// Height of the bottom log drawer. The DAG is the primary surface, so logs
  /// open as a short, resizable, collapsible drawer rather than half the screen.
  const LOGS_KEY = 'nix-web-monitor.logs-height';
  const LOGS_DEFAULT = 220;
  const LOGS_MIN = 80;
  const LOGS_MAX_FRACTION = 0.7;

  const LOGS_COLLAPSED_KEY = 'nix-web-monitor.logs-collapsed';

  let snapshot = $state<MonitorSnapshot>(EMPTY_SNAPSHOT);
  let status = $state<ConnectionStatus>('connecting');
  let sidebarWidth = $state(loadNumber(SIDEBAR_KEY, SIDEBAR_DEFAULT, SIDEBAR_MIN));
  let logsHeight = $state(loadNumber(LOGS_KEY, LOGS_DEFAULT, LOGS_MIN));
  let logsCollapsed = $state(loadBool(LOGS_COLLAPSED_KEY, false));
  let draggingAxis = $state<'horizontal' | 'vertical' | null>(null);
  /// When set, the log drawer filters to entries whose activityId matches this
  /// build's activity. Clicking the same build again or hitting the clear chip
  /// in the log panel resets it.
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

  /// Derivation backing the pinned activity, so the log drawer can name the
  /// build it is filtered to instead of showing a bare activity id.
  const selectedDrv = $derived.by((): string | null => {
    if (selectedActivityId === null) return null;
    return (
      snapshot.builds.find((build) => build.activityId === selectedActivityId)?.derivation ?? null
    );
  });

  function dismissErrors(): void {
    errorsDismissed = snapshot.errors.length;
  }

  /// Selecting a build to inspect its logs also opens the drawer if it was
  /// collapsed, so the filtered lines are actually visible.
  function selectBuild(id: number | null): void {
    selectedActivityId = id;
    if (id !== null && logsCollapsed) setLogsCollapsed(false);
  }

  function setLogsCollapsed(next: boolean): void {
    logsCollapsed = next;
    persistBool(LOGS_COLLAPSED_KEY, next);
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

  function loadBool(key: string, fallback: boolean): boolean {
    if (typeof window === 'undefined') return fallback;
    const stored = window.localStorage.getItem(key);
    if (stored === null) return fallback;
    return stored === 'true';
  }

  function persist(key: string, value: number): void {
    window.localStorage.setItem(key, String(value));
  }

  function persistBool(key: string, value: boolean): void {
    window.localStorage.setItem(key, String(value));
  }

  function clampSidebarWidth(width: number): number {
    const max = Math.max(SIDEBAR_MIN, window.innerWidth * SIDEBAR_MAX_FRACTION);
    return Math.min(max, Math.max(SIDEBAR_MIN, width));
  }

  function clampLogsHeight(height: number): number {
    const max = Math.max(LOGS_MIN, window.innerHeight * LOGS_MAX_FRACTION);
    return Math.min(max, Math.max(LOGS_MIN, height));
  }

  function onPointerMove(event: PointerEvent): void {
    if (draggingAxis === 'horizontal') {
      sidebarWidth = clampSidebarWidth(window.innerWidth - event.clientX);
    } else if (draggingAxis === 'vertical') {
      logsHeight = clampLogsHeight(window.innerHeight - event.clientY);
    }
  }

  function onPointerUp(): void {
    if (draggingAxis === 'horizontal') persist(SIDEBAR_KEY, sidebarWidth);
    else if (draggingAxis === 'vertical') persist(LOGS_KEY, logsHeight);
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

  function logsKeydown(event: KeyboardEvent): void {
    const step = event.shiftKey ? 48 : 16;
    if (event.key === 'ArrowUp') {
      logsHeight = clampLogsHeight(logsHeight + step);
    } else if (event.key === 'ArrowDown') {
      logsHeight = clampLogsHeight(logsHeight - step);
    } else {
      return;
    }
    event.preventDefault();
    persist(LOGS_KEY, logsHeight);
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

  <section
    class="body"
    class:logs-collapsed={logsCollapsed}
    style="--logs-height: {String(logsHeight)}px"
  >
    <section class="workspace" style="--sidebar-width: {String(sidebarWidth)}px">
      <section class="main-pane">
        <BuildTable
          builds={snapshot.builds}
          dependencies={snapshot.dependencies}
          command={snapshot.command}
          expected={snapshot.expected}
          finished={snapshot.finished}
          exitCode={snapshot.exitCode}
          {selectedActivityId}
          onselect={selectBuild}
        />
      </section>
      <Splitter
        orientation="vertical"
        label="Resize activity sidebar"
        valueNow={Math.round(sidebarWidth)}
        onstart={startHorizontal}
        onkeydown={sidebarKeydown}
      />
      <aside class="side-pane">
        <ActivityGraph activities={snapshot.activities} builds={snapshot.builds} />
      </aside>
    </section>

    {#if !logsCollapsed}
      <Splitter
        orientation="horizontal"
        label="Resize log drawer"
        valueNow={Math.round(logsHeight)}
        onstart={startVertical}
        onkeydown={logsKeydown}
      />
    {/if}
    <section class="logs-drawer">
      <LogPanel
        bind:this={logPanel}
        logs={snapshot.logs}
        {selectedActivityId}
        {selectedDrv}
        collapsed={logsCollapsed}
        oncollapse={() => {
          setLogsCollapsed(!logsCollapsed);
        }}
        onclearselection={() => {
          selectedActivityId = null;
        }}
      />
    </section>
  </section>
</main>
