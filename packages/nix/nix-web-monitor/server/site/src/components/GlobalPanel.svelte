<script lang="ts">
  import { SvelteSet } from 'svelte/reactivity';

  import GlobalLogView from '$components/GlobalLogView.svelte';
  import PanelHeader from '$lib/PanelHeader.svelte';
  import { formatDuration, splitDerivation } from '$lib/format';
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

  // ---------- goal-graph forest (graph-capable patched nix) ----------

  /// One coordinator's forest plus the label bits its header shows.
  type CoordinatorView = Readonly<{
    coordinator: GlobalCoordinator;
    key: string;
    label: string;
    forest: GoalForest;
    rows: readonly GoalRow[];
  }>;

  /// Collapsed goals, keyed `<coordinator>:<goal id>` so the same derivation
  /// folding under one coordinator leaves it open under another.
  const collapsed = new SvelteSet<string>();

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

  function toggleCollapse(key: string): void {
    if (collapsed.has(key)) collapsed.delete(key);
    else collapsed.add(key);
  }

  // ---------- flat rows (patched nix without --graph) ----------

  /// The store path a row identifies: the drv for a build, the store path for a
  /// substitution. Either can be null on a drifted entry, so fall back to the
  /// other and finally to a placeholder rather than rendering `null`.
  function pathOf(build: GlobalBuild): string {
    return build.drvPath ?? build.storePath ?? '(unknown)';
  }

  /// Stable row key. The same derivation can be running under two
  /// coordinators, so the path alone would collide.
  function rowKey(build: GlobalBuild): string {
    return `${pathOf(build)}:${String(build.pid ?? 0)}`;
  }

  /// Builds grouped by the client user that requested them, so the panel
  /// answers "who started this" before "what is it". Null users pool under one
  /// unattributed group (a local store without a daemon records no client).
  type UserGroup = Readonly<{
    user: string | null;
    label: string;
    builds: readonly GlobalBuild[];
  }>;

  const groups = $derived(groupByUser(global.builds));

  /// Group headers only earn their row when they attribute something: several
  /// users, or one *known* user. A single anonymous group would just say
  /// "unattributed" above every row.
  const showGroups = $derived(groups.length > 1 || groups.some((group) => group.user !== null));

  function groupByUser(builds: readonly GlobalBuild[]): UserGroup[] {
    const buckets: { user: string | null; label: string; builds: GlobalBuild[] }[] = [];
    for (const build of builds) {
      const bucket = buckets.find((candidate) => candidate.user === build.user);
      if (bucket === undefined) {
        buckets.push({ user: build.user, label: build.user ?? 'unattributed', builds: [build] });
      } else {
        bucket.builds.push(build);
      }
    }
    for (const bucket of buckets) {
      // Oldest first within a group: long-running work floats to the top, and
      // rows keep a stable order across the two-second re-polls.
      bucket.builds.sort((a, b) => (a.startTime ?? 0) - (b.startTime ?? 0));
    }
    return buckets.sort(
      (a, b) => b.builds.length - a.builds.length || a.label.localeCompare(b.label)
    );
  }

  /// One hop of the provenance trail: a derivation shown by name, full path
  /// kept for the tooltip.
  type TrailHop = Readonly<{ path: string; name: string }>;

  /// The ancestors that wanted this goal: the requested root plus the
  /// intermediate hops between it and this goal, in root -> leaf order. Null
  /// when the goal *is* the root (nothing above it to attribute to).
  type Trail = Readonly<{ root: TrailHop; via: readonly TrailHop[] }>;

  function hop(path: string): TrailHop {
    const name = splitDerivation(path).name;
    return { path, name: name.length > 0 ? name : path };
  }

  function whyTrail(build: GlobalBuild): Trail | null {
    // The why-chain is root-first and ends with the goal itself; drop that
    // leaf so the trail is only the ancestors.
    const chain = build.why.chain;
    const ancestors =
      chain.length > 0 && chain[chain.length - 1] === pathOf(build) ? chain.slice(0, -1) : chain;
    const root = ancestors.length > 0 ? ancestors[0] : build.why.rootDrvPath;
    if (root === null || root === pathOf(build)) return null;
    return { root: hop(root), via: ancestors.slice(1).map(hop) };
  }

  /// Label when there is no ancestor trail. A top goal reads "requested
  /// directly"; a sparse entry that still carries a cause shows it verbatim so
  /// the row explains itself either way.
  function noTrailLabel(build: GlobalBuild): string {
    const cause = build.why.cause;
    return cause === null || cause === 'requested' ? 'requested directly' : cause;
  }

  /// Row tooltip: the full store path plus the identity details (outputs,
  /// worker pid, requesting user/uid, cause) that would crowd the row itself.
  function rowTitle(build: GlobalBuild): string {
    const lines = [pathOf(build)];
    if (build.outputs.length > 0) lines.push(`outputs: ${build.outputs.join(', ')}`);
    if (build.pid !== null) lines.push(`worker pid ${String(build.pid)}`);
    if (build.user !== null) {
      lines.push(
        build.uid === null
          ? `requested by ${build.user}`
          : `requested by ${build.user} (uid ${String(build.uid)})`
      );
    }
    if (build.why.cause !== null) lines.push(`cause: ${build.why.cause}`);
    return lines.join('\n');
  }

  /// Trail tooltip: the full why-chain, one store path per line, root first.
  function trailTitle(build: GlobalBuild): string {
    if (build.why.chain.length > 0) return build.why.chain.join('\n→ ');
    return build.why.rootDrvPath ?? '';
  }

  /// Which row's log drawer is open, by row key. One at a time keeps the panel
  /// compact; clicking the open row's toggle closes it.
  let openLog = $state<string | null>(null);

  function toggleLog(key: string): void {
    openLog = openLog === key ? null : key;
  }
</script>

{#if global.detected}
  <section class="panel global-panel">
    <PanelHeader title="machine builds">
      <span class="panel-meta">{meta}</span>
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
        {#each groups as group (group.label)}
          {#if showGroups}
            <div class="global-group">
              <span class="global-group-user">{group.label}</span>
              <span class="global-group-count">{group.builds.length}</span>
            </div>
          {/if}
          {#each group.builds as build (rowKey(build))}
            {@const parts = splitDerivation(pathOf(build))}
            {@const trail = whyTrail(build)}
            <div class="global-row">
              <div class="global-row-head" title={rowTitle(build)}>
                <span class="global-badge global-badge-{build.type}">{BADGE[build.type]}</span>
                <span class="global-name">{parts.name.length > 0 ? parts.name : pathOf(build)}</span>
                {#if parts.version.length > 0}<span class="global-version">{parts.version}</span>{/if}
                {#if build.drvPath !== null && build.logFile !== null}
                  <button
                    type="button"
                    class="global-log-toggle"
                    class:open={openLog === rowKey(build)}
                    aria-expanded={openLog === rowKey(build)}
                    onclick={() => {
                      toggleLog(rowKey(build));
                    }}
                  >
                    log
                  </button>
                {/if}
                <span class="global-elapsed">{elapsed(build.startTime)}</span>
              </div>
              <div class="global-why" title={trailTitle(build)}>
                {#if trail === null}
                  <span class="global-requested">{noTrailLabel(build)}</span>
                {:else}
                  <span class="global-why-label">for</span>
                  <span class="global-why-root" title={trail.root.path}>{trail.root.name}</span>
                  {#if trail.via.length > 0}
                    <span class="global-why-via"
                      >via {trail.via.map((step) => step.name).join(' → ')}</span
                    >
                  {/if}
                {/if}
              </div>
              {#if build.drvPath !== null && openLog === rowKey(build)}
                <GlobalLogView drvPath={build.drvPath} />
              {/if}
            </div>
          {/each}
        {/each}
      {/if}
    </div>
  </section>
{/if}
