<script lang="ts">
  import { SvelteSet } from 'svelte/reactivity';
  import GlobalTreeRow from '$components/GlobalTreeRow.svelte';
  import PanelHeader from '$lib/PanelHeader.svelte';
  import { buildGlobalForest } from '$lib/global-forest';
  import { useNow } from '$lib/now.svelte';
  import type { GlobalBuild, GlobalBuilds } from '$lib/types';

  type Props = {
    global: GlobalBuilds;
  };

  const { global }: Props = $props();

  const now = useNow();

  /// The why-chain forest: active goals hang under the derivations that
  /// requested them, with skeleton nodes for the intermediate hops. Nodes are
  /// keyed by store path, so row identity and expansion state survive the
  /// two-second re-polls.
  const forest = $derived(buildGlobalForest(global.builds));

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

  /// Which goal's log drawer is open, keyed `<path>:<pid>`. One at a time
  /// keeps the panel compact; clicking the open row's toggle closes it.
  let openLog = $state<string | null>(null);

  function toggleLog(key: string): void {
    openLog = openLog === key ? null : key;
  }
</script>

{#if global.detected}
  <section class="panel global-panel">
    <PanelHeader title="machine builds">
      <span class="panel-meta" title={META_TITLE}>{meta}</span>
    </PanelHeader>

    <div class="global-body">
      {#if global.builds.length === 0}
        <div class="global-status">no machine builds right now</div>
      {:else}
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
      {/if}
    </div>
  </section>
{/if}
