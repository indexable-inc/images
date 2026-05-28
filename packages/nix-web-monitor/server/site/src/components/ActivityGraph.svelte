<script lang="ts">
  import type { ActivityNode, BuildNode } from '../types';

  type Props = {
    activities: ActivityNode[];
    builds: BuildNode[];
  };

  const { activities, builds }: Props = $props();

  const buildIds = $derived(new Set(builds.map((build) => build.activityId)));
  const rows = $derived(
    activities
      .slice()
      .sort((left, right) => left.startedTick - right.startedTick)
      .map((activity) => ({
        activity,
        depth: depthFor(activity, activities),
        isBuild: buildIds.has(activity.id)
      }))
  );

  function depthFor(activity: ActivityNode, all: ActivityNode[]): number {
    let depth = 0;
    let parent = activity.parent;
    while (parent !== null && depth < 8) {
      const next = all.find((candidate) => candidate.id === parent);
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
