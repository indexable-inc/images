<script lang="ts">
  import type { SvelteSet } from 'svelte/reactivity';
  import Self from '$components/ActivityTreeRow.svelte';
  import { formatBytes, formatDuration, formatRate, middleTruncate } from '$lib/format';
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

  function percent(progress: NonNullable<ActivityRowMeta['progress']>): number {
    return Math.min(100, Math.round((progress.done / progress.expected) * 100));
  }

  /// Average transfer rate (total bytes moved / elapsed) for a running byte row.
  /// `null` for item-count rows, finished rows, or a span too short to divide,
  /// where an instantaneous figure would just be noise.
  function byteRate(row: ActivityRowMeta): number | null {
    if (row.progress === null || row.progress.unit !== 'bytes' || row.stoppedAtMs !== null) {
      return null;
    }
    const seconds = (now - row.startedAtMs) / 1000;
    return seconds >= 0.5 ? row.progress.done / seconds : null;
  }

  /// Trailing readout next to the bar: `12.4 MB / 80 MB` plus a live rate for an
  /// in-flight download, or `3 / 10` for an item-count activity.
  function progressLabel(row: ActivityRowMeta): string {
    const progress = row.progress;
    if (progress === null) return '';
    if (progress.unit !== 'bytes') {
      return `${String(progress.done)} / ${String(progress.expected)}`;
    }
    const moved = `${formatBytes(progress.done)} / ${formatBytes(progress.expected)}`;
    const rate = byteRate(row);
    return rate === null ? moved : `${moved} · ${formatRate(rate)}`;
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
    {#if meta.count > 1}
      <span class="group-count" title="{String(meta.count)} identical activities folded into one row"
        >×{String(meta.count)}</span
      >
    {/if}
    {#if isCollapsed && children.length > 0}
      <span class="subtree-count">+{String(children.length)}</span>
    {/if}
    {#if meta.progress !== null}
      <span class="activity-progress" title={progressLabel(meta)}>
        <span class="pbar" aria-hidden="true"
          ><span class="pbar-fill" style="--p: {String(percent(meta.progress))}%"></span></span
        >
        <span class="pbar-text">{progressLabel(meta)}</span>
      </span>
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
