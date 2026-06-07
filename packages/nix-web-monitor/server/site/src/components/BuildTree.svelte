<script lang="ts">
  import type { SvelteSet } from 'svelte/reactivity';
  import Self from '$components/BuildTree.svelte';
  import { formatDuration, shortHash, splitDerivation } from '$lib/format';
  import { durationLabel, whereLabel } from '$lib/build-row';
  import { ROOT_SENTINEL, type BuildTree } from '$lib/build-tree';
  import type { BuildNode } from '$lib/types';

  type Props = {
    drv: string;
    tree: BuildTree;
    collapsed: SvelteSet<string>;
    ontoggle: (drv: string) => void;
    now: number;
    selectedActivityId: number | null;
    onselect: (activityId: number | null) => void;
    /// Keyboard cursor: the row vim navigation currently sits on, highlighted
    /// independently of the click selection that drives the log filter.
    cursor: string | null;
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
    cursor,
    guideLines,
    isLast,
    isRoot,
    ancestors
  }: Props = $props();

  const isCommandRoot = $derived(drv === ROOT_SENTINEL);
  const node = $derived(tree.nodeByDrv.get(drv));
  const parts = $derived(splitDerivation(drv));
  const children = $derived((tree.childrenByDrv.get(drv) ?? []).filter((dep) => !ancestors.has(dep)));
  const isCollapsed = $derived(collapsed.has(drv));
  const isCursor = $derived(drv === cursor);
  const childGuideLines = $derived(isRoot ? [] : [...guideLines, !isLast]);
  const childAncestors = $derived(new Set([...ancestors, drv]));

  /// Elapsed wall time for the whole build: live while anything runs or waits,
  /// frozen at the last finish once everything is terminal.
  const rootElapsed = $derived.by((): string => {
    const { startedAtMs, stoppedAtMs, running, planned } = tree.summary;
    if (startedAtMs === null) return '';
    const inFlight = running > 0 || planned > 0;
    const end = inFlight || stoppedAtMs === null ? now : stoppedAtMs;
    return formatDuration(Math.max(0, end - startedAtMs));
  });

  function toggleSelect(build: BuildNode): void {
    if (build.activityId === null) return;
    onselect(selectedActivityId === build.activityId ? null : build.activityId);
  }
</script>

{#if isCommandRoot}
  {@const summary = tree.summary}
  <div class="activity-row root-row" class:cursor={isCursor}>
    <button
      type="button"
      class="twirl"
      class:hidden={children.length === 0}
      aria-label={isCollapsed ? 'expand all' : 'collapse all'}
      aria-expanded={children.length === 0 ? undefined : !isCollapsed}
      tabindex={children.length === 0 ? -1 : 0}
      onclick={() => {
        ontoggle(drv);
      }}
    >
      {children.length === 0 ? '' : isCollapsed ? '▸' : '▾'}
    </button>
    <span class="root-cmd" title={tree.command}>{tree.command.length > 0 ? tree.command : 'build'}</span>
    <span class="root-stats">
      {#if summary.failed > 0}<span class="stat failed">{summary.failed} failed</span>{/if}
      {#if summary.running > 0}<span class="stat running">{summary.running} running</span>{/if}
      <span class="stat done" title="succeeded / total">
        {summary.succeeded}<span class="stat-sep">/</span>{summary.total}
      </span>
    </span>
    <span class="activity-dur">{rootElapsed}</span>
  </div>
{:else if node !== undefined}
  {@const selected = node.activityId !== null && node.activityId === selectedActivityId}
  {@const clickable = node.activityId !== null}
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    class="activity-row dep-row"
    class:stopped={node.status === 'stopped'}
    class:planned={node.status === 'planned'}
    class:clickable
    class:selected
    class:cursor={isCursor}
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
    <span class="guides" aria-hidden="true"
      >{#each guideLines as line, level (level)}<span class="guide">{line ? '│' : ' '}</span
        >{/each}<span class="guide connector">{isLast ? '└' : '├'}</span></span
    >
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
      <span class="drv-name">{parts.name}</span>{#if parts.version.length > 0}<span
          class="drv-version">{parts.version}</span
        >{/if}{#if parts.hash.length > 0}<span class="drv-hash">{shortHash(parts)}</span
        >{/if}{#if node.contentAddressed}<span class="drv-ca" title="content-addressed: Nix resolved this derivation before building it">ca</span
        >{/if}
    </span>
    {#if node.status !== 'planned'}
      <span
        class="where"
        class:remote={whereLabel(node.host) !== 'local'}
        title={whereLabel(node.host)}
      >
        {whereLabel(node.host)}
      </span>
    {/if}
    {#if node.phase !== null}
      <span class="phase">{node.phase}</span>
    {/if}
    {#if isCollapsed && children.length > 0}
      <span class="subtree-count">+{String(children.length)}</span>
    {/if}
    <span class="activity-dur">{durationLabel(node, now)}</span>
  </div>
{/if}

{#if (isCommandRoot || node !== undefined) && !isCollapsed}
  {#each children as childDrv, index (childDrv)}
    <Self
      drv={childDrv}
      {tree}
      {collapsed}
      {ontoggle}
      {now}
      {selectedActivityId}
      {onselect}
      {cursor}
      guideLines={childGuideLines}
      isLast={index === children.length - 1}
      isRoot={false}
      ancestors={childAncestors}
    />
  {/each}
{/if}
