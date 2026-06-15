<script lang="ts">
  // One namespace row, rendered recursively: a container (dict/list/object) shows a
  // caret and expands its `children` inline, each child the same row one level
  // deeper. Leaves have no caret. Open state is local, so expanding one branch
  // never disturbs another.
  import { fmtSize, detail, type NsRow as Row } from '$lib/namespace';
  import KindIcon from './KindIcon.svelte';
  // Svelte 5 expresses recursion by importing the component itself rather than the
  // old <svelte:self>.
  import Self from './NsRow.svelte';

  let { row, depth = 0 }: { row: Row; depth?: number } = $props();

  const hasChildren = $derived(!!row.children && row.children.length > 0);
  let open = $state(false);

  function toggle(): void {
    if (hasChildren) open = !open;
  }
</script>

<div class="nsrow">
  <button
    class="nsrow-line"
    class:has-children={hasChildren}
    style="padding-left: {12 + depth * 15}px"
    onclick={toggle}
    aria-expanded={hasChildren ? open : undefined}
  >
    <span class="nsrow-caret" class:open class:hidden={!hasChildren}>›</span>
    <KindIcon kind={row.kind} />
    <span class="nsrow-name" title={row.type}>{row.name}</span>
    <span class="nsrow-detail" title={detail(row)}>{detail(row)}</span>
    <span class="nsrow-size">{fmtSize(row.size)}</span>
  </button>

  {#if open && row.children}
    {#each row.children as child, i (child.name + ':' + i)}
      <Self row={child} depth={depth + 1} />
    {/each}
  {/if}
</div>

<style>
  .nsrow-line {
    display: grid;
    grid-template-columns: 14px 16px minmax(0, auto) minmax(0, 1fr) auto;
    align-items: center;
    column-gap: 10px;
    width: 100%;
    padding: 5px 12px;
    font: inherit;
    font-family: var(--mono);
    font-size: 12px;
    text-align: left;
    color: var(--ink);
    background: none;
    border: 0;
    border-radius: 7px;
    cursor: default;
    transition: background 0.12s ease;
  }
  .nsrow-line.has-children {
    cursor: pointer;
  }
  .nsrow-line.has-children:hover {
    background: var(--panel);
  }
  /* The caret: a quiet chevron that rotates open. Hidden (but space-preserving) on
     leaves so every row's chip/name column stays aligned. */
  .nsrow-caret {
    justify-self: center;
    color: var(--ink-faint);
    transition: transform 0.12s ease;
    transform: rotate(0deg);
    user-select: none;
  }
  .nsrow-caret.open {
    transform: rotate(90deg);
  }
  .nsrow-caret.hidden {
    visibility: hidden;
  }
  .nsrow-name {
    min-width: 0;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .nsrow-detail {
    min-width: 0;
    color: var(--ink-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .nsrow-size {
    text-align: right;
    color: var(--ink-dim);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
</style>
