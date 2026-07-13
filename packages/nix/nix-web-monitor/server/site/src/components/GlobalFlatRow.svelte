<script lang="ts">
  /// One machine build as a flat (non-tree) row, for the machine panel's
  /// sorted views: badge, name, requesting user, sampled cpu/memory, log
  /// toggle, elapsed. Mirrors the tree row's language minus the guides.

  import GlobalLogView from '$components/GlobalLogView.svelte';
  import { formatBytes, formatDuration, shortHash, splitDerivation } from '$lib/format';
  import { goalKey, goalPath, goalTitle } from '$lib/global-forest';
  import type { GlobalBuild, GlobalBuildKind } from '$lib/types';

  type Props = {
    goal: GlobalBuild;
    now: number;
    openLog: string | null;
    ontogglelog: (key: string) => void;
  };

  const { goal, now, openLog, ontogglelog }: Props = $props();

  const BADGE: Record<GlobalBuildKind, string> = {
    build: 'build',
    substitution: 'sub',
    other: 'other'
  };

  const path = $derived(goalPath(goal));
  const parts = $derived(splitDerivation(path));
  const key = $derived(goalKey(goal));

  const elapsed = $derived(
    goal.startTime === null ? '' : formatDuration(now - goal.startTime * 1000)
  );
</script>

<div class="activity-row global-flat-row" title={goalTitle(goal)}>
  <span class="global-badge global-badge-{goal.type}">{BADGE[goal.type]}</span>
  <span class="drv activity-drv" title={path}>
    <span class="drv-name">{parts.name.length > 0 ? parts.name : path}</span>{#if parts.version.length > 0}<span
        class="drv-version">{parts.version}</span
      >{/if}{#if parts.hash.length > 0}<span class="drv-hash">{shortHash(parts)}</span>{/if}
  </span>
  {#if goal.user !== null}
    <span class="global-user" title="requested by {goal.user}">{goal.user}</span>
  {/if}
  {#if goal.cpuPercent !== null}
    <span class="global-stat" title="cpu across the builder's process tree"
      >{String(goal.cpuPercent)}%</span
    >
  {/if}
  {#if goal.rssBytes !== null}
    <span class="global-stat" title="resident memory across the builder's process tree"
      >{formatBytes(goal.rssBytes)}</span
    >
  {/if}
  {#if goal.drvPath !== null && goal.pid !== null && goal.startTime !== null && goal.startTicks !== null && goal.logFile !== null}
    <button
      type="button"
      class="global-log-toggle"
      class:open={openLog === key}
      aria-expanded={openLog === key}
      onclick={() => {
        ontogglelog(key);
      }}
    >
      log
    </button>
  {/if}
  <span class="activity-dur">{elapsed}</span>
</div>

{#if goal.drvPath !== null && goal.pid !== null && goal.startTime !== null && goal.startTicks !== null && openLog === key}
  <GlobalLogView
    drvPath={goal.drvPath}
    pid={goal.pid}
    startTime={goal.startTime}
    startTicks={goal.startTicks}
  />
{/if}
