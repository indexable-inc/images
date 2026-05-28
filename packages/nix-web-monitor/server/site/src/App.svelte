<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import ActivityGraph from './components/ActivityGraph.svelte';
  import BuildTable from './components/BuildTable.svelte';
  import LogPanel from './components/LogPanel.svelte';
  import SummaryBar from './components/SummaryBar.svelte';
  import { openMonitorEvents } from './monitor-store';
  import { EMPTY_SNAPSHOT, type ConnectionStatus, type MonitorSnapshot } from './types';

  let snapshot = $state<MonitorSnapshot>(EMPTY_SNAPSHOT);
  let status = $state<ConnectionStatus>('connecting');
  let closeEvents: (() => void) | null = null;

  onMount(() => {
    closeEvents = openMonitorEvents(
      (nextSnapshot) => {
        snapshot = nextSnapshot;
      },
      (nextStatus) => {
        status = nextStatus;
      }
    );
  });

  onDestroy(() => {
    closeEvents?.();
  });
</script>

<main>
  <SummaryBar {snapshot} {status} />

  <section class="workspace">
    <ActivityGraph activities={snapshot.activities} builds={snapshot.builds} />
    <BuildTable builds={snapshot.builds} />
    <LogPanel logs={snapshot.logs} />
  </section>
</main>
