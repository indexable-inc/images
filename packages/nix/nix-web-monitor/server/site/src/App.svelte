<script lang="ts">
  import { onDestroy, onMount, type Snippet } from 'svelte';
  import ActivationPanel from '$components/ActivationPanel.svelte';
  import ActivityGraph from '$components/ActivityGraph.svelte';
  import BuildTable from '$components/BuildTable.svelte';
  import DaemonPanel from '$components/DaemonPanel.svelte';
  import DiffPanel from '$components/DiffPanel.svelte';
  import ErrorPanel from '$components/ErrorPanel.svelte';
  import GlobalPanel from '$components/GlobalPanel.svelte';
  import LogPanel from '$components/LogPanel.svelte';
  import SummaryBar from '$components/SummaryBar.svelte';
  import PaneDock from '$lib/panes/PaneDock.svelte';
  import { group, split, type DockLayout, type PaneSpec } from '$lib/panes/types';
  import { openMonitorEvents } from '$lib/monitor-store';
  import { EMPTY_SNAPSHOT, type ConnectionStatus, type MonitorSnapshot } from '$lib/types';

  /// Where the pane system persists the operator's arrangement.
  const LAYOUT_KEY = 'nix-web-monitor.pane-layout';

  let snapshot = $state<MonitorSnapshot>(EMPTY_SNAPSHOT);
  let status = $state<ConnectionStatus>('connecting');
  /// When set, the log pane filters to entries whose activityId matches this
  /// build's activity. Clicking the same build again or hitting the clear chip
  /// in the log pane resets it.
  let selectedActivityId = $state<number | null>(null);
  /// Log panel instance, used to drive its filter from the errors panel. Typed
  /// to the imperative surface we call so the binding stays checked rather than
  /// collapsing to `any`.
  type LogPanelApi = { inspect: (text: string) => void };
  let logPanel = $state<LogPanelApi | null>(null);
  /// Pane dock instance, same pattern: the imperative surface only.
  type DockApi = { resetLayout: () => void; reveal: (id: string) => void };
  let dock = $state<DockApi | null>(null);
  /// Number of errors the operator has dismissed; the panel reappears only when
  /// a newer error pushes the count past this watermark.
  let errorsDismissed = $state(0);
  let closeEvents: (() => void) | null = null;

  const showErrors = $derived(snapshot.errors.length > errorsDismissed);

  /// Derivation backing the pinned activity, so the log pane can name the
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

  /// Selecting a build to inspect its logs also surfaces the logs pane
  /// (activates its tab, expands its group, raises its window), so the
  /// filtered lines are actually visible.
  function selectBuild(id: number | null): void {
    selectedActivityId = id;
    if (id !== null) dock?.reveal('logs');
  }

  function inspectError(text: string): void {
    logPanel?.inspect(text);
    dock?.reveal('logs');
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

  /// Default arrangement: the build DAG is the primary surface, the machine /
  /// activation / changes views share a tab group top-right over the
  /// activities/daemon group, and logs ride a full-width drawer along the
  /// bottom. Only used until the operator rearranges (persisted per browser),
  /// or after "reset layout".
  function defaultLayout(): DockLayout {
    return {
      root: split(
        'column',
        [0.75, 0.25],
        [
          split(
            'row',
            [0.68, 0.32],
            [
              group(['builds']),
              split(
                'column',
                [0.45, 0.55],
                [group(['machine', 'activation', 'changes']), group(['activities', 'daemon'])]
              )
            ]
          ),
          group(['logs'])
        ]
      ),
      floating: []
    };
  }

  /// The pane registry. Conditional visibility mirrors the old hardcoded
  /// layout: `machine` only on a patched nix, `activation` only during a
  /// switch, `changes` only once the post-switch diff lands.
  function paneSpecs(
    builds: Snippet,
    machine: Snippet,
    activation: Snippet,
    changes: Snippet,
    activities: Snippet,
    daemon: Snippet,
    logs: Snippet
  ): PaneSpec[] {
    return [
      { id: 'builds', title: 'builds', content: builds },
      { id: 'machine', title: 'machine', content: machine, visible: snapshot.global.detected },
      {
        id: 'activation',
        title: 'activation',
        content: activation,
        visible: snapshot.activation.active
      },
      { id: 'changes', title: 'changes', content: changes, visible: snapshot.diff !== null },
      { id: 'activities', title: 'activities', content: activities },
      { id: 'daemon', title: 'daemon', content: daemon },
      { id: 'logs', title: 'logs', content: logs }
    ];
  }
</script>

{#snippet buildsPane()}
  <BuildTable
    builds={snapshot.builds}
    dependencies={snapshot.dependencies}
    rootCauses={snapshot.rootCauses}
    rebuildReasons={snapshot.rebuildReasons}
    command={snapshot.command}
    expected={snapshot.expected}
    finished={snapshot.finished}
    exitCode={snapshot.exitCode}
    {selectedActivityId}
    onselect={selectBuild}
  />
{/snippet}

{#snippet machinePane()}
  <GlobalPanel global={snapshot.global} />
{/snippet}

{#snippet activationPane()}
  <ActivationPanel activation={snapshot.activation} />
{/snippet}

{#snippet changesPane()}
  <DiffPanel diff={snapshot.diff} />
{/snippet}

{#snippet activitiesPane()}
  <ActivityGraph activities={snapshot.activities} builds={snapshot.builds} />
{/snippet}

{#snippet daemonPane()}
  <DaemonPanel daemon={snapshot.daemon} />
{/snippet}

{#snippet logsPane()}
  <LogPanel
    bind:this={logPanel}
    logs={snapshot.logs}
    {selectedActivityId}
    {selectedDrv}
    onclearselection={() => {
      selectedActivityId = null;
    }}
  />
{/snippet}

<main>
  <div class="topbar">
    <div class="topbar-row">
      <SummaryBar {snapshot} {status} />
      <button
        type="button"
        class="layout-reset"
        title="restore the default pane arrangement"
        onclick={() => dock?.resetLayout()}
      >
        reset layout
      </button>
    </div>
    {#if showErrors}
      <ErrorPanel errors={snapshot.errors} onclose={dismissErrors} oninspect={inspectError} />
    {/if}
  </div>

  <PaneDock
    bind:this={dock}
    storageKey={LAYOUT_KEY}
    {defaultLayout}
    panes={paneSpecs(
      buildsPane,
      machinePane,
      activationPane,
      changesPane,
      activitiesPane,
      daemonPane,
      logsPane
    )}
  />
</main>
