<script lang="ts">
  import type { SvelteSet } from 'svelte/reactivity';
  import Self from '$components/ActivityTreeRow.svelte';
  import { formatDuration, middleTruncate } from '$lib/format';
  import type { ActivityRowMeta } from '$lib/activity-tree';

  type Props = {
    id: number;
    rowMeta: ReadonlyMap<number, ActivityRowMeta>;
    childrenById: ReadonlyMap<number, readonly number[]>;
    collapsed: SvelteSet<number>;
    ontoggle: (id: number) => void;
    now: number;
    /// Vertical-line flags for each ancestor column (true = ancestor has a
    /// following sibling, so its column keeps a `│`). Empty for roots.
    guideLines: boolean[];
    /// Whether this node is the last among its siblings (picks `└` vs `├`).
    isLast: boolean;
    isRoot: boolean;
  };

  const { id, rowMeta, childrenById, collapsed, ontoggle, now, guideLines, isLast, isRoot }: Props =
    $props();

  const meta = $derived(rowMeta.get(id));
  const children = $derived(childrenById.get(id) ?? []);
  const isCollapsed = $derived(collapsed.has(id));
  const childGuideLines = $derived(isRoot ? [] : [...guideLines, !isLast]);

  function elapsed(row: ActivityRowMeta): number {
    return Math.max(0, (row.stoppedAtMs ?? now) - row.startedAtMs);
  }
</script>

{#if meta !== undefined}
  <div class="activity-row" class:stopped={meta.state === 'stopped'}>
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
      onclick={() => {
        ontoggle(id);
      }}
    >
      {children.length === 0 ? '' : isCollapsed ? '▸' : '▾'}
    </button>
    <span class="state" data-state={meta.state} title={meta.state}></span>
    <span class="activity-kind">{meta.kind}</span>
    {#if meta.derivation !== null}
      <span class="drv activity-drv" title={meta.derivation.hash + meta.derivation.name}>
        <span class="drv-hash">{meta.derivation.hash}</span><span class="drv-name"
          >{meta.derivation.name}</span
        >
      </span>
    {:else}
      <span class="activity-text" title={meta.text}>{middleTruncate(meta.text, 80)}</span>
    {/if}
    {#if isCollapsed && children.length > 0}
      <span class="subtree-count">+{String(children.length)}</span>
    {/if}
    <span class="activity-dur">{formatDuration(elapsed(meta))}</span>
  </div>

  {#if !isCollapsed}
    {#each children as childId, index (childId)}
      <Self
        id={childId}
        {rowMeta}
        {childrenById}
        {collapsed}
        {ontoggle}
        {now}
        guideLines={childGuideLines}
        isLast={index === children.length - 1}
        isRoot={false}
      />
    {/each}
  {/if}
{/if}
