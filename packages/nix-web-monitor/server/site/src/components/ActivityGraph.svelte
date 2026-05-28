<script lang="ts">
  import type { ActivityNode, BuildNode } from '../types';

  type Props = {
    activities: ReadonlyArray<ActivityNode>;
    builds: ReadonlyArray<BuildNode>;
  };

  const { activities, builds }: Props = $props();

  const MAX_DEPTH = 8;

  const buildActivityIds = $derived(
    new Set(builds.flatMap((build) => (build.activityId === null ? [] : [build.activityId])))
  );

  const byId = $derived(new Map(activities.map((activity) => [activity.id, activity])));

  const rows = $derived(
    activities
      .toSorted((left, right) => left.startedTick - right.startedTick)
      .map((activity) => ({
        activity,
        depth: depthFor(activity, byId),
        isBuild: buildActivityIds.has(activity.id)
      }))
  );

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
</script>

<section class="panel graph-panel">
  <div class="panel-title">activity dag</div>
  <div class="graph">
    {#each rows as row (row.activity.id)}
      <div
        class="activity-row"
        class:build={row.isBuild}
        class:stopped={row.activity.status === 'stopped'}
        style={`--depth: ${String(row.depth)}`}
      >
        <span class="join" aria-hidden="true"></span>
        <span class="activity-kind">{row.activity.activityType.name}</span>
        <span class="activity-text">{row.activity.phase ?? row.activity.text}</span>
      </div>
    {:else}
      <div class="empty">waiting for events</div>
    {/each}
  </div>
</section>
