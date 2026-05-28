<script lang="ts">
  import { SvelteSet } from 'svelte/reactivity';
  import PanelHeader from '$lib/PanelHeader.svelte';
  import BuildTree from '$components/BuildTree.svelte';
  import { splitDerivation, formatDuration } from '$lib/format';
  import { buildDependencyTree } from '$lib/build-tree';
  import { useNow } from '$lib/now.svelte';
  import {
    ACTIVITY_NAME_BUILD,
    type BuildNode,
    type BuildStatus,
    type DerivationEdge
  } from '$lib/types';

  type Props = {
    builds: BuildNode[];
    dependencies: DerivationEdge[];
    expected: Record<string, number>;
    /// Whether the Nix run has exited. Flips the empty placeholder from a
    /// "waiting" message to a terminal one so a finished run with no builds
    /// does not look like it is still pending.
    finished: boolean;
    /// Process exit code once finished; distinguishes "nothing to build" (0)
    /// from "stopped before any build" (non-zero, e.g. an eval failure).
    exitCode: number | null;
    selectedActivityId: number | null;
    onselect: (activityId: number | null) => void;
  };

  const { builds, dependencies, expected, finished, exitCode, selectedActivityId, onselect }: Props =
    $props();

  const now = useNow();

  /// Running first, then failed, stopped, and finally succeeded; ties broken by
  /// derivation path. Shared by the flat list and the dependency tree so both
  /// views rank builds the same way.
  const STATUS_ORDER: Record<BuildStatus, number> = {
    running: 0,
    failed: 1,
    stopped: 2,
    succeeded: 3
  };

  function compareBuilds(left: BuildNode, right: BuildNode): number {
    const byStatus = STATUS_ORDER[left.status] - STATUS_ORDER[right.status];
    return byStatus !== 0 ? byStatus : left.derivation.localeCompare(right.derivation);
  }

  /// Tree view nests builds by dependency; flat view is the plain sorted list.
  let layout = $state<'flat' | 'tree'>('flat');

  const ordered = $derived(builds.toSorted(compareBuilds));
  const tree = $derived(buildDependencyTree(builds, dependencies, compareBuilds));
  const collapsed = new SvelteSet<string>();

  const expectedBuilds = $derived(expected[ACTIVITY_NAME_BUILD] ?? 0);

  /// Placeholder shown when no build rows exist. A finished run gets a terminal
  /// message (everything substituted, or stopped before building) instead of a
  /// "waiting" one that wrongly implies work is still pending.
  const emptyLabel = $derived.by((): string => {
    if (finished) return exitCode === 0 ? 'nothing to build' : 'stopped before building';
    if (expectedBuilds > 0) {
      return `waiting for ${String(expectedBuilds)} build${expectedBuilds === 1 ? '' : 's'}`;
    }
    return 'waiting for build phase';
  });

  function elapsedMs(build: BuildNode): number {
    const end = build.stoppedAtMs ?? now.value;
    return Math.max(0, end - build.startedAtMs);
  }

  function whereLabel(host: string | null): string {
    if (host === null || host.length === 0) return 'local';
    return host;
  }

  function whereIsRemote(host: string | null): boolean {
    return host !== null && host.length > 0;
  }

  function toggleSelect(build: BuildNode): void {
    if (build.activityId === null) return;
    onselect(selectedActivityId === build.activityId ? null : build.activityId);
  }
</script>

<section class="panel builds-panel">
  <PanelHeader title="builds">
    <div class="filter-chips">
      <button type="button" class="chip" class:active={layout === 'flat'} onclick={() => (layout = 'flat')}>
        flat
      </button>
      <button
        type="button"
        class="chip"
        class:active={layout === 'tree'}
        title={tree.hasEdges ? 'nest builds by dependency' : 'no dependency edges resolved yet'}
        onclick={() => (layout = 'tree')}
      >
        tree
      </button>
    </div>
    <span class="panel-meta">
      {String(builds.length)}{#if expectedBuilds > 0} / {String(expectedBuilds)}{/if}
    </span>
  </PanelHeader>
  <div class="build-table" class:tree={layout === 'tree'}>
    {#if layout === 'tree'}
      {#each tree.roots as rootDrv, index (rootDrv)}
        <BuildTree
          drv={rootDrv}
          {tree}
          {collapsed}
          ontoggle={(drv: string) => {
            if (collapsed.has(drv)) collapsed.delete(drv);
            else collapsed.add(drv);
          }}
          now={now.value}
          {selectedActivityId}
          {onselect}
          guideLines={[]}
          isLast={index === tree.roots.length - 1}
          isRoot={true}
          ancestors={new Set()}
        />
      {:else}
        <div class="empty wide">{emptyLabel}</div>
      {/each}
    {:else}
      {#each ordered as build (build.derivation)}
        {@const parts = splitDerivation(build.derivation)}
        {@const selected = build.activityId !== null && build.activityId === selectedActivityId}
        <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
        <div
          class="build-row"
          class:selected
          class:clickable={build.activityId !== null}
          role={build.activityId !== null ? 'button' : undefined}
          tabindex={build.activityId !== null ? 0 : undefined}
          aria-pressed={build.activityId !== null ? selected : undefined}
          onclick={() => { toggleSelect(build); }}
          onkeydown={(event) => {
            if (event.key === 'Enter' || event.key === ' ') {
              event.preventDefault();
              toggleSelect(build);
            }
          }}
        >
          <div class="state" data-state={build.status} title={build.status}></div>
          <div class="drv" title={build.derivation}>
            <span class="drv-hash">{parts.hash}</span><span class="drv-name">{parts.name}</span>
          </div>
          <div class="where" class:remote={whereIsRemote(build.host)} title={whereLabel(build.host)}>
            {whereLabel(build.host)}
          </div>
          <div class="phase">{build.phase ?? ''}</div>
          <div class="duration">{formatDuration(elapsedMs(build))}</div>
          <div class="right">{String(build.logCount)}</div>
        </div>
      {:else}
        <div class="empty wide">{emptyLabel}</div>
      {/each}
    {/if}
  </div>
</section>
