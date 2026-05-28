<script lang="ts">
  import PanelHeader from '$lib/PanelHeader.svelte';
  import type { ActivityNode, BuildNode } from '$lib/types';

  type Props = {
    activities: ActivityNode[];
    builds: BuildNode[];
  };

  const { activities, builds }: Props = $props();

  const MAX_DEPTH = 8;

  const buildActivityIds = $derived(
    new Set(builds.flatMap((build) => (build.activityId === null ? [] : [build.activityId])))
  );

  const byId = $derived(new Map(activities.map((activity) => [activity.id, activity])));

  const rows = $derived(
    activities
      .filter((activity) => visibleRow(activity, buildActivityIds))
      .toSorted((left, right) => left.startedTick - right.startedTick)
      .map((activity) => ({
        activity,
        depth: depthFor(activity, byId),
        isBuild: buildActivityIds.has(activity.id),
        kind: kindLabel(activity),
        display: rowText(activity)
      }))
  );

  const hiddenCount = $derived(activities.length - rows.length);

  function visibleRow(activity: ActivityNode, buildIds: ReadonlySet<number>): boolean {
    // Build activities are always interesting, even if Nix gives them empty
    // text. Everything else needs phase or text to be worth a row.
    if (buildIds.has(activity.id)) return true;
    return activity.text.length > 0 || activity.phase !== null;
  }

  function rowText(activity: ActivityNode): string {
    if (activity.phase !== null) return activity.phase;
    return activity.text;
  }

  /// Nix tags many real activities with type `unknown` (code 0). The text
  /// usually leads with an action verb (\"evaluating\", \"copying\",
  /// \"querying\", \"downloading\"). Synthesize the kind label from that
  /// verb so the column actually classifies the row.
  function kindLabel(activity: ActivityNode): string {
    const declared = activity.activityType.name;
    if (declared !== 'unknown') return declared;
    const verb = /^([a-zA-Z]+)/.exec(activity.text)?.[1];
    return verb === undefined ? 'note' : verb.toLowerCase();
  }

  function depthFor(activity: ActivityNode, lookup: ReadonlyMap<number, ActivityNode>): number {
    let depth = 0;
    let parent = activity.parent;
    while (parent !== null && depth < MAX_DEPTH) {
      const next = lookup.get(parent);
      if (next === undefined) break;
      depth += 1;
      parent = next.parent;
    }
    return depth;
  }

  /// Keep both the head and tail of long strings visible. Most rows are file
  /// paths and the tail (filename) is more identifying than the prefix.
  function middleTruncate(text: string, max: number): string {
    if (text.length <= max) return text;
    const head = Math.ceil((max - 1) / 2);
    const tail = Math.floor((max - 1) / 2);
    return `${text.slice(0, head)}…${text.slice(text.length - tail)}`;
  }
</script>

<section class="panel graph-panel">
  <PanelHeader title="activities">
    <span class="panel-meta">
      {String(rows.length)} shown{#if hiddenCount > 0} &middot; {String(hiddenCount)} hidden{/if}
    </span>
  </PanelHeader>
  <div class="graph">
    {#each rows as row (row.activity.id)}
      <div
        class="activity-row"
        class:build={row.isBuild}
        class:stopped={row.activity.status === 'stopped'}
        style="--depth: {String(row.depth)}"
        title={row.display}
      >
        <span class="activity-kind">{row.kind}</span>
        <span class="activity-text">{middleTruncate(row.display, 80)}</span>
      </div>
    {:else}
      <div class="empty">waiting for events</div>
    {/each}
  </div>
</section>
