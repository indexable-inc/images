<script lang="ts">
  import PanelHeader from '$lib/PanelHeader.svelte';
  import { formatDuration, splitDerivation } from '$lib/format';
  import { useNow } from '$lib/now.svelte';
  import type { GlobalBuild, GlobalBuilds } from '$lib/types';

  type Props = {
    global: GlobalBuilds;
  };

  const { global }: Props = $props();

  const now = useNow();

  /// The store path a row identifies: the drv for a build, the store path for a
  /// substitution. Either can be null on a drifted entry, so fall back to the
  /// other and finally to a placeholder rather than rendering `null`.
  function pathOf(build: GlobalBuild): string {
    return build.drvPath ?? build.storePath ?? '(unknown)';
  }

  /// Short 'build' / 'sub' badge for the goal kind. Anything the C++ side adds
  /// beyond the two known kinds is shown verbatim rather than dropped.
  function badge(type: string): string {
    if (type === 'build') return 'build';
    if (type === 'substitution') return 'sub';
    return type;
  }

  /// Live elapsed label from the goal's start. `startTime` is unix *seconds*
  /// (unlike the rest of the monitor's ms timestamps), so scale to ms before
  /// diffing against the reactive clock. Empty when the source gave no start.
  function elapsed(startTimeSec: number | null): string {
    if (startTimeSec === null) return '';
    return formatDuration(now.value - startTimeSec * 1000);
  }

  /// Compact why-chain: the derivation names from the requested root down to this
  /// goal, joined by arrows (`app → foo`). Falls back to the root alone, then to
  /// the cause, so a row always explains itself even on a sparse entry.
  function whyChain(build: GlobalBuild): string {
    const chain = build.why.chain;
    if (chain.length > 0) {
      return chain.map((path) => splitDerivation(path).name).join(' → ');
    }
    if (build.why.rootDrvPath !== null) {
      return splitDerivation(build.why.rootDrvPath).name;
    }
    return build.why.cause ?? '';
  }
</script>

{#if global.detected}
  <section class="panel global-panel">
    <PanelHeader title="machine builds">
      <span class="panel-meta">{global.builds.length}</span>
    </PanelHeader>

    <div class="global-body">
      {#if global.builds.length === 0}
        <div class="global-status">no machine builds right now</div>
      {:else}
        {#each global.builds as build (pathOf(build))}
          {@const parts = splitDerivation(pathOf(build))}
          <div class="global-row" title={pathOf(build)}>
            <div class="global-row-head">
              <span class="global-badge global-badge-{build.type}">{badge(build.type)}</span>
              <span class="global-name">{parts.name || pathOf(build)}</span>
              {#if parts.version}<span class="global-version">{parts.version}</span>{/if}
              <span class="global-elapsed">{elapsed(build.startTime)}</span>
            </div>
            <div class="global-row-meta">
              {#if build.user !== null}<span class="global-user">{build.user}</span>{/if}
              {#if build.why.cause !== null}<span class="global-cause">{build.why.cause}</span>{/if}
            </div>
            {#if whyChain(build)}
              <div class="global-why" title={build.why.rootDrvPath ?? ''}>{whyChain(build)}</div>
            {/if}
            {#if build.logFile !== null}
              <div class="global-log" title={build.logFile}>{build.logFile}</div>
            {/if}
          </div>
        {/each}
      {/if}
    </div>
  </section>
{/if}
