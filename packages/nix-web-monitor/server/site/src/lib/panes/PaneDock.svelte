<script lang="ts">
  /// The pane system's root: give it the registered panes and a default
  /// layout, and it renders the split/tab tree plus any floating windows,
  /// persisting every arrangement change under `storageKey`.
  ///
  /// The host keeps a `bind:this` reference for the imperative surface:
  /// `resetLayout()` (the reset-layout affordance) and `reveal(id)` (bring a
  /// pane to the front, e.g. "selecting a build shows the logs").

  import { untrack } from 'svelte';
  import PaneGroup from '$lib/panes/PaneGroup.svelte';
  import PaneSplit from '$lib/panes/PaneSplit.svelte';
  import PaneWindow from '$lib/panes/PaneWindow.svelte';
  import { setDockContext } from '$lib/panes/context';
  import { DockState } from '$lib/panes/layout.svelte';
  import type { DockLayout, PaneId, PaneSpec } from '$lib/panes/types';

  type Props = {
    panes: PaneSpec[];
    storageKey: string;
    defaultLayout: () => DockLayout;
  };

  const { panes, storageKey, defaultLayout }: Props = $props();

  // The storage key and default factory are constructor-time configuration on
  // purpose: a dock does not re-key or re-default mid-life.
  // svelte-ignore state_referenced_locally
  const dock = new DockState(storageKey, defaultLayout);
  const specs = $derived(new Map(panes.map((pane) => [pane.id, pane])));

  let root = $state<HTMLElement | null>(null);

  // Keep the layout consistent with the registered pane set (drops stale ids
  // from a persisted layout, slots new panes in). Track only the id list;
  // reconcile itself reads *and* rewrites the layout tree, which inside a
  // tracked scope would re-trigger this effect forever.
  $effect(() => {
    const ids = panes.map((pane) => pane.id);
    untrack(() => {
      dock.reconcile(ids);
    });
  });

  setDockContext({
    state: dock,
    spec: (id: PaneId) => specs.get(id),
    hidden: (id: PaneId) => specs.get(id)?.visible === false,
    dockElement: () => root
  });

  export function resetLayout(): void {
    dock.reset();
  }

  export function reveal(id: PaneId): void {
    dock.reveal(id);
  }
</script>

<div class="pane-dock" bind:this={root}>
  {#if dock.layout.root.kind === 'split'}
    <PaneSplit split={dock.layout.root} />
  {:else}
    <PaneGroup group={dock.layout.root} />
  {/if}
  {#each dock.layout.floating as floating (floating.id)}
    <PaneWindow {floating} />
  {/each}
</div>

<style>
  .pane-dock {
    position: relative;
    min-width: 0;
    min-height: 0;
    display: grid;
    background: var(--bg, #f8fafc);
    overflow: hidden;
  }

  /* The single grid cell hosts the root node; floating windows are absolutely
   * positioned siblings layered above it in DOM order. */
  .pane-dock > :global(.pane-split),
  .pane-dock > :global(.pane-group) {
    grid-area: 1 / 1;
    min-width: 0;
    min-height: 0;
  }
</style>
