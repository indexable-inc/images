<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import ResourceUsage from './lib/ResourceUsage.svelte';
  import { FALLBACK_STATS, parseUsageStats, type Status } from './lib/stats';

  let stats = $state(FALLBACK_STATS);
  let status = $state<Status>('connecting');
  let timer = $state<number | null>(null);

  async function refresh() {
    try {
      const response = await fetch('/stats.json', { cache: 'no-store' });
      if (!response.ok) throw new Error(`stats returned ${String(response.status)}`);
      const nextStats = parseUsageStats(await response.json());
      if (nextStats === null) throw new Error('stats response did not match the expected shape');
      stats = nextStats;
      status = 'live';
    } catch {
      status = 'waiting';
    }
  }

  onMount(() => {
    void refresh();
    timer = window.setInterval(refresh, 1000);
  });

  onDestroy(() => {
    if (timer !== null) window.clearInterval(timer);
  });
</script>

<main class="landing">
  <div class="mark">ix</div>

  <ResourceUsage {stats} {status} />
</main>
