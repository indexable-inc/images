<script lang="ts">
  import { SvelteSet } from 'svelte/reactivity';
  import GlobalFlatRow from '$components/GlobalFlatRow.svelte';
  import GlobalTreeRow from '$components/GlobalTreeRow.svelte';
  import { formatBytes } from '$lib/format';
  import { buildGlobalForest, goalKey } from '$lib/global-forest';
  import { useNow } from '$lib/now.svelte';
  import type { GlobalBuild, GlobalBuilds } from '$lib/types';

  type Props = {
    global: GlobalBuilds;
  };

  const { global }: Props = $props();

  const now = useNow();

  /// View orderings. `tree` is the why-chain forest (who wants what); the flat
  /// sorts answer "what is eating this machine" using the server-sampled
  /// per-build stats.
  const SORTS = [
    ['tree', 'why-chain'],
    ['cpu', 'cpu'],
    ['mem', 'memory'],
    ['age', 'elapsed']
  ] as const;
  type SortMode = (typeof SORTS)[number][0];

  const SORT_KEY = 'nix-web-monitor.global-sort';

  let sortMode = $state<SortMode>(loadSort());

  function loadSort(): SortMode {
    if (typeof window === 'undefined') return 'tree';
    const stored = window.localStorage.getItem(SORT_KEY);
    const found = SORTS.find(([mode]) => mode === stored);
    return found === undefined ? 'tree' : found[0];
  }

  function setSort(mode: SortMode): void {
    sortMode = mode;
    window.localStorage.setItem(SORT_KEY, mode);
  }

  /// The why-chain forest: active goals hang under the derivations that
  /// requested them, with skeleton nodes for the intermediate hops. Nodes are
  /// keyed by store path, so row identity and expansion state survive the
  /// two-second re-polls.
  const forest = $derived(buildGlobalForest(global.builds));

  /// Flat orderings put the biggest consumer (or the oldest goal) first;
  /// goals the sampler could not measure sink to the bottom.
  const sorted = $derived.by((): GlobalBuild[] => {
    const goals = [...global.builds];
    if (sortMode === 'cpu') {
      goals.sort((a, b) => (b.cpuPercent ?? -1) - (a.cpuPercent ?? -1));
    } else if (sortMode === 'mem') {
      goals.sort((a, b) => (b.rssBytes ?? -1) - (a.rssBytes ?? -1));
    } else {
      goals.sort(
        (a, b) => (a.startTime ?? Number.MAX_SAFE_INTEGER) - (b.startTime ?? Number.MAX_SAFE_INTEGER)
      );
    }
    return goals;
  });

  const meta = $derived(countsLabel(global.builds));

  /// The chains cover only active goals plus their ancestors, so the forest
  /// under-reports: waiting or queued sibling derivations are invisible until
  /// the status dir records the full goal graph. Surfaced as the meta tooltip
  /// so the panel does not read as the whole plan.
  const META_TITLE =
    'active goals and their ancestor chains only; waiting or queued siblings are not recorded yet';

  /// Collapsed forest nodes. Store-path keys stay stable while goals come and
  /// go, so a fold survives updates to the underlying list.
  const collapsed = new SvelteSet<string>();

  function toggle(path: string): void {
    if (collapsed.has(path)) collapsed.delete(path);
    else collapsed.add(path);
  }

  /// Header meta: the build/fetch mix, plus machine-wide cpu/memory totals
  /// when the sampler has figures ("3 building · 2 fetching · 412% · 3.1 GB").
  function countsLabel(builds: readonly GlobalBuild[]): string {
    const building = builds.filter((build) => build.type === 'build').length;
    const fetching = builds.filter((build) => build.type === 'substitution').length;
    const other = builds.length - building - fetching;
    const parts: string[] = [];
    if (building > 0) parts.push(`${String(building)} building`);
    if (fetching > 0) parts.push(`${String(fetching)} fetching`);
    if (other > 0) parts.push(`${String(other)} other`);
    const cpu = builds.reduce((sum, build) => sum + (build.cpuPercent ?? 0), 0);
    const rss = builds.reduce((sum, build) => sum + (build.rssBytes ?? 0), 0);
    if (cpu > 0) parts.push(`${String(cpu)}%`);
    if (rss > 0) parts.push(formatBytes(rss));
    return parts.length === 0 ? 'idle' : parts.join(' · ');
  }

  /// Which goal's log drawer is open, keyed `<path>:<pid>`. One at a time
  /// keeps the panel compact; clicking the open row's toggle closes it.
  let openLog = $state<string | null>(null);

  function toggleLog(key: string): void {
    openLog = openLog === key ? null : key;
  }
</script>

<section class="panel global-panel">
  <div class="pane-toolbar">
    <div class="filter-chips" role="tablist" aria-label="machine builds ordering">
      {#each SORTS as [mode, label] (mode)}
        <button
          type="button"
          class="chip"
          class:active={sortMode === mode}
          onclick={() => {
            setSort(mode);
          }}
        >
          {label}
        </button>
      {/each}
    </div>
    <span class="panel-meta" title={META_TITLE}>{meta}</span>
  </div>

  <div class="global-body">
    {#if !global.detected}
      <div class="global-status">{global.status}</div>
    {:else if global.builds.length === 0}
      <div class="global-status">no machine builds right now</div>
    {:else if sortMode === 'tree'}
      <div class="global-tree">
        {#each forest.roots as root, index (root)}
          <GlobalTreeRow
            path={root}
            {forest}
            {collapsed}
            ontoggle={toggle}
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
    {:else}
      <div class="global-tree">
        {#each sorted as goal (goalKey(goal))}
          <GlobalFlatRow {goal} now={now.value} {openLog} ontogglelog={toggleLog} />
        {/each}
      </div>
    {/if}
  </div>
</section>
