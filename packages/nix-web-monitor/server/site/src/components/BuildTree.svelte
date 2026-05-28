<script lang="ts">
  import type { SvelteSet } from 'svelte/reactivity';
  import Self from '$components/BuildTree.svelte';
  import { formatDuration, splitDerivation } from '$lib/format';
  import type { BuildTree } from '$lib/build-tree';
  import type { BuildNode } from '$lib/types';

  type Props = {
    drv: string;
    tree: BuildTree;
    collapsed: SvelteSet<string>;
    ontoggle: (drv: string) => void;
    now: number;
    selectedActivityId: number | null;
    onselect: (activityId: number | null) => void;
    /// Vertical-line flags for each ancestor column (true = ancestor has a
    /// following sibling, so its column keeps a `│`). Empty for roots.
    guideLines: boolean[];
    /// Whether this node is the last among its siblings (picks `└` vs `├`).
    isLast: boolean;
    isRoot: boolean;
    /// Derivations on the path from the root to here. Guards against re-entering
    /// a node already above us, which a malformed cyclic edge set could induce.
    ancestors: ReadonlySet<string>;
  };

  const {
    drv,
    tree,
    collapsed,
    ontoggle,
    now,
    selectedActivityId,
    onselect,
    guideLines,
    isLast,
    isRoot,
    ancestors
  }: Props = $props();

  const node = $derived(tree.nodeByDrv.get(drv));
  const parts = $derived(splitDerivation(drv));
  const children = $derived((tree.childrenByDrv.get(drv) ?? []).filter((dep) => !ancestors.has(dep)));
  const isCollapsed = $derived(collapsed.has(drv));
  const childGuideLines = $derived(isRoot ? [] : [...guideLines, !isLast]);
  const childAncestors = $derived(new Set([...ancestors, drv]));

  function elapsedMs(build: BuildNode): number {
    return Math.max(0, (build.stoppedAtMs ?? now) - build.startedAtMs);
  }

  function whereLabel(host: string | null): string {
    return host === null || host.length === 0 ? 'local' : host;
  }

  function toggleSelect(build: BuildNode): void {
    if (build.activityId === null) return;
    onselect(selectedActivityId === build.activityId ? null : build.activityId);
  }
</script>

{#if node !== undefined}
  {@const selected = node.activityId !== null && node.activityId === selectedActivityId}
  {@const clickable = node.activityId !== null}
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    class="activity-row dep-row"
    class:stopped={node.status === 'stopped'}
    class:clickable
    class:selected
    role={clickable ? 'button' : undefined}
    tabindex={clickable ? 0 : undefined}
    aria-pressed={clickable ? selected : undefined}
    onclick={() => {
      toggleSelect(node);
    }}
    onkeydown={(event) => {
      if (clickable && (event.key === 'Enter' || event.key === ' ')) {
        event.preventDefault();
        toggleSelect(node);
      }
    }}
  >
    {#if !isRoot}
      <span class="guides" aria-hidden="true"
        >{#each guideLines as line, level (level)}<span class="guide">{line ? '│' : ' '}</span
          >{/each}<span class="guide connector">{isLast ? '└' : '├'}</span></span
      >
    {/if}
    <button
      type="button"
      class="twirl"
      class:hidden={children.length === 0}
      aria-label={isCollapsed ? 'expand' : 'collapse'}
      aria-expanded={children.length === 0 ? undefined : !isCollapsed}
      tabindex={children.length === 0 ? -1 : 0}
      onclick={(event) => {
        event.stopPropagation();
        ontoggle(drv);
      }}
    >
      {children.length === 0 ? '' : isCollapsed ? '▸' : '▾'}
    </button>
    <span class="state" data-state={node.status} title={node.status}></span>
    <span class="drv activity-drv" title={drv}>
      <span class="drv-hash">{parts.hash}</span><span class="drv-name">{parts.name}</span>
    </span>
    <span class="where" class:remote={whereLabel(node.host) !== 'local'} title={whereLabel(node.host)}>
      {whereLabel(node.host)}
    </span>
    {#if node.phase !== null}
      <span class="phase">{node.phase}</span>
    {/if}
    {#if isCollapsed && children.length > 0}
      <span class="subtree-count">+{String(children.length)}</span>
    {/if}
    <span class="activity-dur">{formatDuration(elapsedMs(node))}</span>
  </div>

  {#if !isCollapsed}
    {#each children as childDrv, index (childDrv)}
      <Self
        drv={childDrv}
        {tree}
        {collapsed}
        {ontoggle}
        {now}
        {selectedActivityId}
        {onselect}
        guideLines={childGuideLines}
        isLast={index === children.length - 1}
        isRoot={false}
        ancestors={childAncestors}
      />
    {/each}
  {/if}
{/if}
