<script lang="ts">
  import { SvelteSet } from 'svelte/reactivity';
  import PanelHeader from '$lib/PanelHeader.svelte';
  import BuildTree from '$components/BuildTree.svelte';
  import { shortHash, splitDerivation } from '$lib/format';
  import { durationLabel, isRemote, whereLabel } from '$lib/build-row';
  import { buildDependencyTree, flattenVisible, hasChildren, ROOT_SENTINEL } from '$lib/build-tree';
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
    /// The Nix invocation, shown as the tree's single root label.
    command: string;
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

  const {
    builds,
    dependencies,
    command,
    expected,
    finished,
    exitCode,
    selectedActivityId,
    onselect
  }: Props = $props();

  const now = useNow();

  /// Running first, then failed, stopped, and finally succeeded; ties broken by
  /// derivation path. Shared by the flat list and the dependency tree so both
  /// views rank builds the same way.
  const STATUS_ORDER: Record<BuildStatus, number> = {
    running: 0,
    failed: 1,
    stopped: 2,
    succeeded: 3,
    planned: 4
  };

  function compareBuilds(left: BuildNode, right: BuildNode): number {
    const byStatus = STATUS_ORDER[left.status] - STATUS_ORDER[right.status];
    return byStatus !== 0 ? byStatus : left.derivation.localeCompare(right.derivation);
  }

  /// Tree view nests builds by dependency; flat view is the plain sorted list.
  /// The tree is the default: with the plan seeding every node up front it shows
  /// the target at the root and its whole subtree, lighting up as builds run.
  let layout = $state<'flat' | 'tree'>('tree');

  const ordered = $derived(builds.toSorted(compareBuilds));
  const tree = $derived(buildDependencyTree(builds, dependencies, compareBuilds, command));
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

  function toggleSelect(build: BuildNode): void {
    if (build.activityId === null) return;
    onselect(selectedActivityId === build.activityId ? null : build.activityId);
  }

  // ---- vim-style keyboard navigation -------------------------------------
  // The build panel is the primary surface, so it owns the navigation keys
  // (j/k/h/l, gg/G, o/Enter). The log drawer keeps only `/` and Esc, so the two
  // window handlers never fight over a key.

  /// The row the keyboard cursor sits on, highlighted separately from the click
  /// selection that drives the log filter. Null until the first key moves it.
  let cursor = $state<string | null>(null);
  let tableEl = $state<HTMLDivElement | null>(null);
  /// First half of a pending `gg`. Reset by any other key.
  let awaitingG = $state(false);

  /// The rows currently on screen, in display order: the flattened tree, or the
  /// plain sorted list in flat layout. This is what the cursor steps through.
  const visibleRows = $derived(
    layout === 'tree'
      ? flattenVisible(tree, collapsed)
      : ordered.map((build) => build.derivation)
  );

  /// The cursor, snapped to a row that still exists (a build finishing or a
  /// collapse can drop the row out from under it). Defaults to the first row.
  const cursorDrv = $derived.by((): string | null => {
    if (visibleRows.length === 0) return null;
    if (cursor !== null && visibleRows.includes(cursor)) return cursor;
    return visibleRows[0];
  });

  function moveCursor(delta: number): void {
    if (visibleRows.length === 0) return;
    const current = cursorDrv === null ? 0 : visibleRows.indexOf(cursorDrv);
    const next = Math.min(visibleRows.length - 1, Math.max(0, current + delta));
    cursor = visibleRows[next];
  }

  /// `h`: collapse an open node, else step to its parent. Leaves the cursor put
  /// at the very top.
  function collapseOrParent(): void {
    const drv = cursorDrv;
    if (drv === null || layout !== 'tree') return;
    if (hasChildren(tree, drv) && !collapsed.has(drv)) {
      collapsed.add(drv);
      return;
    }
    const parent = tree.parentByDrv.get(drv);
    if (parent !== undefined) cursor = parent;
  }

  /// `l`: expand a collapsed node, else descend to its first child.
  function expandOrChild(): void {
    const drv = cursorDrv;
    if (drv === null || layout !== 'tree' || !hasChildren(tree, drv)) return;
    if (collapsed.has(drv)) {
      collapsed.delete(drv);
      return;
    }
    const first = (tree.childrenByDrv.get(drv) ?? []).at(0);
    if (first !== undefined) cursor = first;
  }

  /// `o`/Enter: select the cursor's build to pin the log drawer to it. The
  /// command root has no logs of its own, so it is a no-op there.
  function selectCursor(): void {
    const drv = cursorDrv;
    if (drv === null || drv === ROOT_SENTINEL) return;
    const id = tree.nodeByDrv.get(drv)?.activityId ?? null;
    if (id !== null) onselect(selectedActivityId === id ? null : id);
  }

  function onWindowKeydown(event: KeyboardEvent): void {
    const target = event.target;
    const typing =
      target instanceof HTMLElement &&
      (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable);
    if (typing || event.metaKey || event.ctrlKey || event.altKey) return;

    // `gg` jumps to the first row; the first `g` just arms the second.
    if (event.key === 'g') {
      if (awaitingG) {
        cursor = visibleRows.at(0) ?? null;
        awaitingG = false;
        event.preventDefault();
      } else {
        awaitingG = true;
      }
      return;
    }
    awaitingG = false;

    switch (event.key) {
      case 'j':
      case 'ArrowDown':
        moveCursor(1);
        break;
      case 'k':
      case 'ArrowUp':
        moveCursor(-1);
        break;
      case 'G':
        cursor = visibleRows.at(-1) ?? null;
        break;
      case 'h':
      case 'ArrowLeft':
        collapseOrParent();
        break;
      case 'l':
      case 'ArrowRight':
        expandOrChild();
        break;
      case 'o':
      case 'Enter':
        selectCursor();
        break;
      default:
        return;
    }
    event.preventDefault();
  }

  /// Keep the cursor row scrolled into view as it moves under keyboard control.
  $effect(() => {
    void cursorDrv;
    const row = tableEl?.querySelector('.cursor');
    if (row instanceof HTMLElement) row.scrollIntoView({ block: 'nearest' });
  });
</script>

<svelte:window onkeydown={onWindowKeydown} />

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
  <div class="build-table" class:tree={layout === 'tree'} bind:this={tableEl}>
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
          cursor={cursorDrv}
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
          class:planned={build.status === 'planned'}
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
            <span class="drv-name">{parts.name}</span>{#if parts.version.length > 0}<span
                class="drv-version">{parts.version}</span
              >{/if}{#if parts.hash.length > 0}<span class="drv-hash"
                >{shortHash(parts)}</span
              >{/if}{#if build.contentAddressed}<span class="drv-ca"
                title="content-addressed: Nix resolved this derivation before building it">ca</span
              >{/if}
          </div>
          <div class="where" class:remote={isRemote(build.host)} title={whereLabel(build.host)}>
            {whereLabel(build.host)}
          </div>
          <div class="phase">{build.phase ?? ''}</div>
          <div class="duration">{durationLabel(build, now.value)}</div>
          <div class="right">{String(build.logCount)}</div>
        </div>
      {:else}
        <div class="empty wide">{emptyLabel}</div>
      {/each}
    {/if}
  </div>
</section>
