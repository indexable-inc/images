<script lang="ts">
  /// Recursive renderer for a split node: children on one axis with a
  /// draggable splitter between each adjacent pair. Children whose subtree has
  /// nothing visible (every pane hidden by the host) are skipped entirely and
  /// their space redistributed; collapsed groups take only their natural tab
  /// bar size, and the splitters beside them go inert.

  import Self from '$lib/panes/PaneSplit.svelte';
  import PaneGroup from '$lib/panes/PaneGroup.svelte';
  import PaneSplitter from '$lib/panes/PaneSplitter.svelte';
  import { getDockContext } from '$lib/panes/context';
  import { nodeHasVisible } from '$lib/panes/layout.svelte';
  import type { LayoutNode, SplitNode } from '$lib/panes/types';

  type Props = {
    split: SplitNode;
  };

  const { split }: Props = $props();
  const dock = getDockContext();

  let container = $state<HTMLElement | null>(null);

  type VisibleChild = { child: LayoutNode; index: number };

  const visible = $derived(
    split.children
      .map((child, index): VisibleChild => ({ child, index }))
      .filter(({ child }) => nodeHasVisible(child, dock.hidden))
  );

  function isCollapsed(child: LayoutNode): boolean {
    return child.kind === 'group' && child.collapsed;
  }

  /// Sum of the size fractions of visible, expanded children: the `fr` space
  /// the browser actually distributes, and the scale for drag deltas.
  const expandedShare = $derived(
    visible.reduce(
      (sum, { child, index }) => (isCollapsed(child) ? sum : sum + (split.sizes.at(index) ?? 0)),
      0
    )
  );

  /// One CSS grid holds children and splitters alike: `fr` tracks for expanded
  /// children (the browser renormalizes when some are hidden), `auto` for
  /// collapsed tab bars, fixed 5px tracks for splitters.
  const template = $derived.by(() => {
    const tracks: string[] = [];
    visible.forEach(({ child, index }, position) => {
      if (position > 0) tracks.push('5px');
      tracks.push(
        isCollapsed(child) ? 'auto' : `minmax(0, ${String(split.sizes.at(index) ?? 1)}fr)`
      );
    });
    return tracks.join(' ');
  });

  /// Pixel extent of the split along its axis, for delta-to-fraction math.
  function extentPx(): number {
    if (container === null) return 1;
    const extent = split.direction === 'row' ? container.clientWidth : container.clientHeight;
    return Math.max(1, extent);
  }

  /// A drag on the splitter after visible child `position` shifts size between
  /// that child and the next *visible* sibling -- the two panes the rendered
  /// splitter actually sits between, which are not adjacent in the stored
  /// children when a hidden sibling lies between them. Deltas arrive in
  /// pixels; the state layer works in fractions of the whole split, so scale
  /// by the currently distributed share (hidden siblings keep their stored
  /// fractions and regain them when they reappear).
  function dragBetween(position: number, deltaPx: number): void {
    const left = visible.at(position);
    const right = visible.at(position + 1);
    if (left === undefined || right === undefined) return;
    const fraction = (deltaPx / extentPx()) * expandedShare;
    dock.state.resizeSplit(split, left.index, right.index, fraction);
  }

  function keyStep(position: number, sign: -1 | 1, big: boolean): void {
    dragBetween(position, sign * (big ? 48 : 16));
    dock.state.persist();
  }

  function valuePercent(position: number): number {
    const entry = visible.at(position);
    if (entry === undefined || expandedShare <= 0) return 50;
    return ((split.sizes.at(entry.index) ?? 0) / expandedShare) * 100;
  }

  function splitterDisabled(position: number): boolean {
    const left = visible.at(position);
    const right = visible.at(position + 1);
    if (left === undefined || right === undefined) return true;
    return isCollapsed(left.child) || isCollapsed(right.child);
  }
</script>

<div
  class="pane-split {split.direction}"
  style="{split.direction === 'row' ? 'grid-template-columns' : 'grid-template-rows'}: {template}"
  bind:this={container}
>
  {#each visible as { child }, position (child)}
    {#if position > 0}
      <PaneSplitter
        direction={split.direction}
        label="Resize panes"
        valuePercent={valuePercent(position - 1)}
        disabled={splitterDisabled(position - 1)}
        ondrag={(deltaPx: number) => {
          dragBetween(position - 1, deltaPx);
        }}
        ondragend={() => {
          dock.state.persist();
        }}
        onkeystep={(sign: -1 | 1, big: boolean) => {
          keyStep(position - 1, sign, big);
        }}
      />
    {/if}
    {#if child.kind === 'split'}
      <Self split={child} />
    {:else}
      <PaneGroup group={child} parentDirection={split.direction} />
    {/if}
  {/each}
</div>

<style>
  .pane-split {
    display: grid;
    min-width: 0;
    min-height: 0;
    background: var(--bg, #f8fafc);
  }
</style>
