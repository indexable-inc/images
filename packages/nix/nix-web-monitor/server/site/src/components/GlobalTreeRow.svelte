<script lang="ts">
  import type { SvelteSet } from 'svelte/reactivity';
  import Self from '$components/GlobalTreeRow.svelte';
  import GlobalLogView from '$components/GlobalLogView.svelte';
  import { formatDuration, shortHash, splitDerivation } from '$lib/format';
  import { goalPath, type GlobalForest } from '$lib/global-forest';
  import type { GlobalBuild, GlobalBuildKind } from '$lib/types';

  type Props = {
    path: string;
    forest: GlobalForest;
    collapsed: SvelteSet<string>;
    ontoggle: (path: string) => void;
    now: number;
    /// Which goal's log drawer is open, keyed `<path>:<pid>`. One at a time
    /// across the whole panel keeps it compact.
    openLog: string | null;
    ontogglelog: (key: string) => void;
    /// Vertical-line flags for each ancestor column (true = ancestor has a
    /// following sibling, so its column keeps a `│`). Empty for roots.
    guideLines: boolean[];
    /// Whether this node is the last among its siblings (picks `└` vs `├`).
    isLast: boolean;
    isRoot: boolean;
    /// Paths from the root to here. Guards against re-entering a node already
    /// above us, which contradictory why-chains could induce.
    ancestors: ReadonlySet<string>;
  };

  const {
    path,
    forest,
    collapsed,
    ontoggle,
    now,
    openLog,
    ontogglelog,
    guideLines,
    isLast,
    isRoot,
    ancestors
  }: Props = $props();

  /// Short badge per goal kind. The Rust side already folds unknown kinds into
  /// `other`, so this record is total.
  const BADGE: Record<GlobalBuildKind, string> = {
    build: 'build',
    substitution: 'sub',
    other: 'other'
  };

  const goals = $derived(forest.goalsByPath.get(path) ?? []);
  /// The goal whose affordances the row carries. The rare extra goals for the
  /// same path (the status dir keys entries by `<path>-<pid>`, one per daemon
  /// worker) fold into a ×N marker. Undefined on a skeleton ancestor.
  const primary = $derived(goals.at(0));
  const parts = $derived(splitDerivation(path));
  const children = $derived(
    (forest.childrenByPath.get(path) ?? []).filter((child) => !ancestors.has(child))
  );
  const isCollapsed = $derived(collapsed.has(path));
  const childGuideLines = $derived(isRoot ? [] : [...guideLines, !isLast]);
  const childAncestors = $derived(new Set([...ancestors, path]));

  /// Stable per-goal key for the log drawer: the status dir keys entries by
  /// `<path>-<pid>`, so the path alone would collide across workers.
  function goalKey(goal: GlobalBuild): string {
    return `${goalPath(goal)}:${String(goal.pid ?? 0)}`;
  }

  /// Live elapsed label from the goal's start. `startTime` is unix *seconds*
  /// (unlike the rest of the monitor's ms timestamps), so scale to ms before
  /// diffing against the reactive clock. Empty when the source gave no start.
  function elapsed(goal: GlobalBuild): string {
    if (goal.startTime === null) return '';
    return formatDuration(now - goal.startTime * 1000);
  }

  /// Row tooltip: the full store path plus the identity details (outputs,
  /// worker pid, requesting user/uid, cause) that would crowd the row itself.
  /// A skeleton hop instead explains why it has no affordances.
  function rowTitle(): string {
    if (primary === undefined) {
      return `${path}\nancestor of an active goal below, not itself active`;
    }
    const lines = [path];
    if (primary.outputs.length > 0) lines.push(`outputs: ${primary.outputs.join(', ')}`);
    if (primary.pid !== null) lines.push(`worker pid ${String(primary.pid)}`);
    if (primary.user !== null) {
      lines.push(
        primary.uid === null
          ? `requested by ${primary.user}`
          : `requested by ${primary.user} (uid ${String(primary.uid)})`
      );
    }
    if (primary.why.cause !== null) lines.push(`cause: ${primary.why.cause}`);
    return lines.join('\n');
  }
</script>

<div class="activity-row global-tree-row" class:skeleton={primary === undefined} title={rowTitle()}>
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
      ontoggle(path);
    }}
  >
    {children.length === 0 ? '' : isCollapsed ? '▸' : '▾'}
  </button>
  {#if primary !== undefined}
    <span class="global-badge global-badge-{primary.type}">{BADGE[primary.type]}</span>
  {/if}
  <span class="drv activity-drv" title={path}>
    <span class="drv-name">{parts.name.length > 0 ? parts.name : path}</span>{#if parts.version.length > 0}<span
        class="drv-version">{parts.version}</span
      >{/if}{#if parts.hash.length > 0}<span class="drv-hash">{shortHash(parts)}</span>{/if}
  </span>
  {#if goals.length > 1}
    <span
      class="group-count"
      title="{String(goals.length)} active goals for this path, one per daemon worker"
      >×{String(goals.length)}</span
    >
  {/if}
  {#if primary !== undefined && primary.user !== null}
    <span class="global-user" title="requested by {primary.user}">{primary.user}</span>
  {/if}
  {#if primary !== undefined && primary.drvPath !== null && primary.logFile !== null}
    <button
      type="button"
      class="global-log-toggle"
      class:open={openLog === goalKey(primary)}
      aria-expanded={openLog === goalKey(primary)}
      onclick={() => {
        ontogglelog(goalKey(primary));
      }}
    >
      log
    </button>
  {/if}
  {#if isCollapsed && children.length > 0}
    <span class="subtree-count">+{String(children.length)}</span>
  {/if}
  <span class="activity-dur">{primary === undefined ? '' : elapsed(primary)}</span>
</div>

{#if primary !== undefined && primary.drvPath !== null && openLog === goalKey(primary)}
  <GlobalLogView drvPath={primary.drvPath} />
{/if}

{#if !isCollapsed}
  {#each children as childPath, index (childPath)}
    <Self
      path={childPath}
      {forest}
      {collapsed}
      {ontoggle}
      {now}
      {openLog}
      {ontogglelog}
      guideLines={childGuideLines}
      isLast={index === children.length - 1}
      isRoot={false}
      ancestors={childAncestors}
    />
  {/each}
{/if}
