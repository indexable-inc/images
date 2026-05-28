<script lang="ts">
  import type { BuildNode, BuildStatus } from '../types';

  type Props = {
    builds: ReadonlyArray<BuildNode>;
  };

  const { builds }: Props = $props();

  const STATUS_ORDER: Readonly<Record<BuildStatus, number>> = {
    running: 0,
    failed: 1,
    stopped: 2,
    succeeded: 3
  };

  const ordered = $derived(
    builds.toSorted((left, right) => {
      const byStatus = STATUS_ORDER[left.status] - STATUS_ORDER[right.status];
      return byStatus !== 0 ? byStatus : left.derivation.localeCompare(right.derivation);
    })
  );

  function shortDrv(path: string): string {
    const slash = path.lastIndexOf('/');
    return slash === -1 ? path : path.slice(slash + 1);
  }
</script>

<section class="panel builds-panel">
  <div class="panel-title">builds</div>
  <div class="build-table">
    <div class="head">state</div>
    <div class="head">derivation</div>
    <div class="head">phase</div>
    <div class="head right">logs</div>
    {#each ordered as build (build.derivation)}
      <div class="state" data-state={build.status}>{build.status}</div>
      <div class="drv" title={build.derivation}>{shortDrv(build.derivation)}</div>
      <div class="phase">{build.phase ?? '-'}</div>
      <div class="right">{String(build.logCount)}</div>
    {:else}
      <div class="empty wide">waiting for builds</div>
    {/each}
  </div>
</section>
