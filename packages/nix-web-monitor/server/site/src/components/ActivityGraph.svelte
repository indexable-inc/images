<script lang="ts">
  import { SvelteSet } from 'svelte/reactivity';
  import PanelHeader from '$lib/PanelHeader.svelte';
  import ActivityTreeRow from '$components/ActivityTreeRow.svelte';
  import { buildActivityTree } from '$lib/activity-tree';
  import { useNow } from '$lib/now.svelte';
  import type { ActivityNode, BuildNode } from '$lib/types';

  type Props = {
    activities: ActivityNode[];
    builds: BuildNode[];
  };

  const { activities, builds }: Props = $props();

  const now = useNow();
  const collapsed = new SvelteSet<number>();

  const tree = $derived(buildActivityTree(activities, builds));
  const collapsible = $derived(
    [...tree.childrenById.entries()].flatMap(([id, kids]) => (kids.length > 0 ? [id] : []))
  );
  const allCollapsed = $derived(collapsible.length > 0 && collapsed.size >= collapsible.length);

  function toggle(id: number): void {
    if (collapsed.has(id)) collapsed.delete(id);
    else collapsed.add(id);
  }

  function toggleAll(): void {
    if (allCollapsed) collapsed.clear();
    else for (const id of collapsible) collapsed.add(id);
  }
</script>

<section class="panel graph-panel">
  <PanelHeader title="activities">
    {#if collapsible.length > 0}
      <button type="button" class="chip tree-toggle" onclick={toggleAll}>
        {allCollapsed ? 'expand all' : 'collapse all'}
      </button>
    {/if}
    <span class="panel-meta">
      {String(tree.shown)} shown{#if tree.hidden > 0} &middot; {String(tree.hidden)} hidden{/if}
    </span>
  </PanelHeader>
  <div class="graph">
    {#each tree.roots as rootId, index (rootId)}
      <ActivityTreeRow
        id={rootId}
        rowMeta={tree.rowMeta}
        childrenById={tree.childrenById}
        {collapsed}
        ontoggle={toggle}
        now={now.value}
        guideLines={[]}
        isLast={index === tree.roots.length - 1}
        isRoot={true}
      />
    {:else}
      <div class="empty">waiting for events</div>
    {/each}
  </div>
</section>
