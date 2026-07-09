<script lang="ts">
  import { SvelteSet } from 'svelte/reactivity';

  import GlobalLogView from '$components/GlobalLogView.svelte';
  import GlobalTreeRow from '$components/GlobalTreeRow.svelte';
  import PanelHeader from '$lib/PanelHeader.svelte';
  import { formatDuration, splitDerivation } from '$lib/format';
  import { buildGlobalForest } from '$lib/global-forest';
  import { buildGoalForest, flattenGoalForest, type GoalForest, type GoalRow } from '$lib/global-tree';
  import { useNow } from '$lib/now.svelte';
  import type {
    GlobalBuild,
    GlobalBuildKind,
    GlobalBuilds,
    GlobalCoordinator,
    GlobalGoal,
    GlobalGoalStatus
  } from '$lib/types';

  type Props = {
    global: GlobalBuilds;
  };

  const { global }: Props = $props();

  const now = useNow();

  /// Short badge per goal kind. The Rust side already folds unknown kinds into
  /// `other`, so this record is total.
  const BADGE: Record<GlobalBuildKind, string> = {
    build: 'build',
    substitution: 'sub',
    other: 'other'
  };

  const meta = $derived(countsLabel(global.builds));

  /// Header meta splitting builds from substitutions, so the mix is readable
  /// without scanning badges ("3 building · 2 fetching").
  function countsLabel(builds: readonly GlobalBuild[]): string {
    const building = builds.filter((build) => build.type === 'build').length;
    const fetching = builds.filter((build) => build.type === 'substitution').length;
    const other = builds.length - building - fetching;
    const parts: string[] = [];
    if (building > 0) parts.push(`${String(building)} building`);
    if (fetching > 0) parts.push(`${String(fetching)} fetching`);
    if (other > 0) parts.push(`${String(other)} other`);
    return parts.length === 0 ? 'idle' : parts.join(' · ');
  }

  /// Live elapsed label from the goal's start. `startTime` is unix *seconds*
  /// (unlike the rest of the monitor's ms timestamps), so scale to ms before
  /// diffing against the reactive clock. Empty when the source gave no start.
  function elapsed(startTimeSec: number | null): string {
    if (startTimeSec === null) return '';
    return formatDuration(now.value - startTimeSec * 1000);
  }

  /// Collapsed nodes across both views. Coordinator-forest goals are keyed
  /// `<coordinator>:<goal id>`; fallback why-chain nodes are keyed by store
  /// path. Both key spaces are disjoint and stable across the two-second
  /// re-polls, so folds survive updates to the underlying lists.
  const collapsed = new SvelteSet<string>();

  function toggleCollapse(key: string): void {
    if (collapsed.has(key)) collapsed.delete(key);
    else collapsed.add(key);
  }

  /// Which goal's log drawer is open. One at a time keeps the panel compact;
  /// clicking the open row's toggle closes it.
  let openLog = $state<string | null>(null);

  function toggleLog(key: string): void {
    openLog = openLog === key ? null : key;
  }

  // ---------- goal-graph forest (graph-capable patched nix) ----------

  /// One coordinator's forest plus the label bits its header shows.
  type CoordinatorView = Readonly<{
    coordinator: GlobalCoordinator;
    key: string;
    label: string;
    forest: GoalForest;
    rows: readonly GoalRow[];
  }>;

  const coordinators = $derived(global.coordinators.map(coordinatorView));

  function coordinatorKey(coordinator: GlobalCoordinator): string {
    return `pid:${String(coordinator.pid ?? 0)}`;
  }

  function coordinatorView(coordinator: GlobalCoordinator): CoordinatorView {
    const key = coordinatorKey(coordinator);
    const forest = buildGoalForest(coordinator);
    const user = coordinator.user ?? 'unattributed';
    const pid = coordinator.pid === null ? '' : ` · pid ${String(coordinator.pid)}`;
    return {
      coordinator,
      key,
      label: `${user}${pid}`,
      forest,
      rows: flattenGoalForest(forest, (id) => collapsed.has(`${key}:${id}`))
    };
  }

  /// Header counts for one coordinator: live work first, then the session's
  /// completed record ("2 running · 3 waiting · 5 done").
  function forestCounts(forest: GoalForest): string {
    const order: readonly GlobalGoalStatus[] = ['running', 'waiting', 'done', 'failed', 'other'];
    const parts = order
      .filter((status) => forest.counts[status] > 0)
      .map((status) => `${String(forest.counts[status])} ${status}`);
    return parts.length === 0 ? 'idle' : parts.join(' · ');
  }

  /// The `.state` dot palette is shared with the invocation build tree, whose
  /// statuses differ; map goal states onto it (waiting renders as the hollow
  /// "pending" ring, done as the success fill).
  const DOT_STATE: Record<GlobalGoalStatus, string> = {
    waiting: 'planned',
    running: 'running',
    done: 'succeeded',
    failed: 'failed',
    other: 'other'
  };

  function goalKey(view: CoordinatorView, goal: GlobalGoal): string {
    return `${view.key}:${goal.id}`;
  }

  /// Goal row tooltip: the full id plus the details that would crowd the row.
  function goalTitle(goal: GlobalGoal): string {
    const lines = [goal.id, goal.status];
    if (goal.outputs.length > 0) lines.push(`outputs: ${goal.outputs.join(', ')}`);
    if (goal.builderPid !== null) lines.push(`builder pid ${String(goal.builderPid)}`);
    return lines.join('\n');
  }

  // ---------- why-chain fallback (patched nix without graph files) ----------

  /// The why-chain forest: active goals hang under the derivations that
  /// requested them, with skeleton nodes for the intermediate hops. Nodes are
  /// keyed by store path, so row identity and expansion state survive the
  /// two-second re-polls.
  const forest = $derived(buildGlobalForest(global.builds));

  /// The chains cover only active goals plus their ancestors, so the fallback
  /// forest under-reports: waiting or queued sibling derivations are invisible
  /// without the goal-graph files. Surfaced as the meta tooltip so the panel
  /// does not read as the whole plan.
  const META_TITLE =
    'active goals and their ancestor chains only; waiting or queued siblings are not recorded yet';

  /// The caveat only applies while the fallback view is what is shown; the
  /// coordinator forest is the whole graph.
  const metaTitle = $derived(global.coordinators.length > 0 ? undefined : META_TITLE);
</script>

{#if global.detected}
  <section class="panel global-panel">
    <PanelHeader title="machine builds">
      <span class="panel-meta" title={metaTitle}>{meta}</span>
    </PanelHeader>

    <div class="global-body">
      {#if global.coordinators.length > 0}
        {#each coordinators as view (view.key)}
          <div class="global-group">
            <span class="global-group-user">{view.label}</span>
            <span class="global-group-count">{forestCounts(view.forest)}</span>
          </div>
          {#each view.rows as row, index (`${goalKey(view, row.goal)}#${String(index)}`)}
            {@const goal = row.goal}
            {@const key = goalKey(view, goal)}
            {@const parts = splitDerivation(goal.id)}
            {@const running = goal.status === 'running'}
            <div
              class="global-goal"
              class:settled={goal.status === 'done' || goal.status === 'failed'}
              style:--goal-depth={row.depth}
            >
              <div class="global-row-head" title={goalTitle(goal)}>
                <button
                  type="button"
                  class="twirl"
                  class:hidden={!row.hasChildren || row.repeat}
                  aria-label={collapsed.has(key) ? 'expand' : 'collapse'}
                  aria-expanded={row.hasChildren && !row.repeat ? !collapsed.has(key) : undefined}
                  tabindex={row.hasChildren && !row.repeat ? 0 : -1}
                  onclick={() => {
                    toggleCollapse(key);
                  }}
                >
                  {!row.hasChildren || row.repeat ? '' : collapsed.has(key) ? '▸' : '▾'}
                </button>
                <span class="state" data-state={DOT_STATE[goal.status]} title={goal.status}></span>
                {#if goal.kind === 'substitution'}
                  <span class="global-badge global-badge-substitution">{BADGE[goal.kind]}</span>
                {/if}
                <span class="global-name">{parts.name.length > 0 ? parts.name : goal.id}</span>
                {#if parts.version.length > 0}<span class="global-version">{parts.version}</span>{/if}
                {#if row.repeat}
                  <span class="global-repeat" title="also shown above under another dependent">↩</span>
                {/if}
                {#if running && goal.kind === 'build' && goal.logFile !== null}
                  <button
                    type="button"
                    class="global-log-toggle"
                    class:open={openLog === key}
                    aria-expanded={openLog === key}
                    onclick={() => {
                      toggleLog(key);
                    }}
                  >
                    log
                  </button>
                {/if}
                {#if running}
                  <span class="global-elapsed">{elapsed(goal.startTime)}</span>
                {/if}
              </div>
              {#if running && goal.kind === 'build' && openLog === key}
                <GlobalLogView drvPath={goal.id} />
              {/if}
            </div>
          {/each}
        {/each}
      {:else if global.builds.length === 0}
        <div class="global-status">no machine builds right now</div>
      {:else}
        <div class="global-tree">
          {#each forest.roots as root, index (root)}
            <GlobalTreeRow
              path={root}
              {forest}
              {collapsed}
              ontoggle={toggleCollapse}
              now={now.value}
              {openLog}
              ontogglelog={toggleLog}
              guideLines={[]}
              isLast={index === forest.roots.length - 1}
              isRoot={true}
              ancestors={new Set()}
            />
          {/each}
        </div>
      {/if}
    </div>
  </section>
{/if}
