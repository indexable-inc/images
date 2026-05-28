<script lang="ts">
  import type { BuildNode } from '../types';

  type Props = {
    builds: BuildNode[];
  };

  const { builds }: Props = $props();

  const ordered = $derived(
    builds
      .slice()
      .sort((left, right) => left.derivation.localeCompare(right.derivation))
      .sort((left, right) => statusRank(left.status) - statusRank(right.status))
  );

  function statusRank(status: BuildNode['status']): number {
    if (status === 'running') return 0;
    if (status === 'failed') return 1;
    return 2;
  }

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
