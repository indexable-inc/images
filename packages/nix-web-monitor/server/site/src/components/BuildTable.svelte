<script lang="ts">
  import { ACTIVITY_NAME_BUILD, type BuildNode, type BuildStatus } from '../types';

  type Props = {
    builds: ReadonlyArray<BuildNode>;
    expected: Readonly<Record<string, number>>;
  };

  const { builds, expected }: Props = $props();

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

  const expectedBuilds = $derived(expected[ACTIVITY_NAME_BUILD] ?? 0);

  function shortDrv(path: string): string {
    const slash = path.lastIndexOf('/');
    return slash === -1 ? path : path.slice(slash + 1);
  }
</script>

<section class="panel builds-panel">
  <header class="panel-title">
    <span>builds</span>
    <span class="panel-meta">{String(builds.length)}{#if expectedBuilds > 0} / {String(expectedBuilds)}{/if}</span>
  </header>
  <div class="build-table">
    {#each ordered as build (build.derivation)}
      <div class="state" data-state={build.status} title={build.status}></div>
      <div class="drv" title={build.derivation}>{shortDrv(build.derivation)}</div>
      <div class="phase">{build.phase ?? ''}</div>
      <div class="right">{String(build.logCount)}</div>
    {:else}
      <div class="empty wide">
        {#if expectedBuilds > 0}
          waiting for {String(expectedBuilds)} build{expectedBuilds === 1 ? '' : 's'}
        {:else}
          waiting for build phase
        {/if}
      </div>
    {/each}
  </div>
</section>
