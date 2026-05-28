<script lang="ts">
  import type { ConnectionStatus, MonitorSnapshot } from '../types';

  type Props = {
    snapshot: MonitorSnapshot;
    status: ConnectionStatus;
  };

  const { snapshot, status }: Props = $props();

  const running = $derived(snapshot.builds.filter((build) => build.status === 'running').length);
  const failed = $derived(snapshot.builds.filter((build) => build.status === 'failed').length);
  const succeeded = $derived(snapshot.builds.filter((build) => build.status === 'succeeded').length);
  const expectedBuilds = $derived(
    Object.hasOwn(snapshot.expected, 'build') ? snapshot.expected.build : snapshot.builds.length
  );
  const exit = $derived(snapshot.exitCode === null ? '' : `exit ${String(snapshot.exitCode)}`);
</script>

<header class="summary">
  <div class="brand">nix-web-monitor</div>
  <div class="metric" data-state={status}>{status}</div>
  <div class="metric">builds {String(snapshot.builds.length)} / {String(expectedBuilds)}</div>
  <div class="metric good">ok {String(succeeded)}</div>
  <div class="metric warn">run {String(running)}</div>
  <div class="metric bad">fail {String(failed)}</div>
  {#if exit !== ''}
    <div class="metric">{exit}</div>
  {/if}
</header>
