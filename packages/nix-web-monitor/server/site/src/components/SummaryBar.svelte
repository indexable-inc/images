<script lang="ts">
  import {
    ACTIVITY_NAME_BUILD,
    type BuildStatus,
    type ConnectionStatus,
    type MonitorSnapshot
  } from '../types';

  type Props = {
    snapshot: MonitorSnapshot;
    status: ConnectionStatus;
  };

  const { snapshot, status }: Props = $props();

  type StatusCounts = Readonly<Record<BuildStatus, number>>;

  const counts = $derived(
    snapshot.builds.reduce<StatusCounts>(
      (acc, build) => ({ ...acc, [build.status]: acc[build.status] + 1 }),
      { running: 0, stopped: 0, succeeded: 0, failed: 0 }
    )
  );

  const expectedBuilds = $derived(
    Object.hasOwn(snapshot.expected, ACTIVITY_NAME_BUILD)
      ? snapshot.expected[ACTIVITY_NAME_BUILD]
      : snapshot.builds.length
  );

  const exit = $derived(snapshot.exitCode === null ? '' : `exit ${String(snapshot.exitCode)}`);
</script>

<header class="summary">
  <div class="brand">nix-web-monitor</div>
  <div class="metric" data-state={status}>{status}</div>
  <div class="metric">builds {String(snapshot.builds.length)} / {String(expectedBuilds)}</div>
  <div class="metric good">ok {String(counts.succeeded)}</div>
  <div class="metric warn">run {String(counts.running)}</div>
  <div class="metric">done {String(counts.stopped)}</div>
  <div class="metric bad">fail {String(counts.failed)}</div>
  {#if exit !== ''}
    <div class="metric">{exit}</div>
  {/if}
</header>
