<script lang="ts">
  import PanelHeader from '$lib/PanelHeader.svelte';
  import { splitDerivation, formatDuration } from '$lib/format';
  import { useNow } from '$lib/now.svelte';
  import { ACTIVITY_NAME_BUILD, type BuildNode, type BuildStatus } from '$lib/types';

  type Props = {
    builds: BuildNode[];
    expected: Record<string, number>;
    selectedActivityId: number | null;
    onselect: (activityId: number | null) => void;
  };

  const { builds, expected, selectedActivityId, onselect }: Props = $props();

  const now = useNow();

  const STATUS_ORDER: Record<BuildStatus, number> = {
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

  function elapsedMs(build: BuildNode): number {
    const end = build.stoppedAtMs ?? now.value;
    return Math.max(0, end - build.startedAtMs);
  }

  function whereLabel(host: string | null): string {
    if (host === null || host.length === 0) return 'local';
    return host;
  }

  function whereIsRemote(host: string | null): boolean {
    return host !== null && host.length > 0;
  }

  function toggleSelect(build: BuildNode): void {
    if (build.activityId === null) return;
    onselect(selectedActivityId === build.activityId ? null : build.activityId);
  }
</script>

<section class="panel builds-panel">
  <PanelHeader title="builds">
    <span class="panel-meta">
      {String(builds.length)}{#if expectedBuilds > 0} / {String(expectedBuilds)}{/if}
    </span>
  </PanelHeader>
  <div class="build-table">
    {#each ordered as build (build.derivation)}
      {@const parts = splitDerivation(build.derivation)}
      {@const selected = build.activityId !== null && build.activityId === selectedActivityId}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <div
        class="build-row"
        class:selected
        class:clickable={build.activityId !== null}
        role={build.activityId !== null ? 'button' : undefined}
        tabindex={build.activityId !== null ? 0 : undefined}
        aria-pressed={build.activityId !== null ? selected : undefined}
        onclick={() => { toggleSelect(build); }}
        onkeydown={(event) => {
          if (event.key === 'Enter' || event.key === ' ') {
            event.preventDefault();
            toggleSelect(build);
          }
        }}
      >
        <div class="state" data-state={build.status} title={build.status}></div>
        <div class="drv" title={build.derivation}>
          <span class="drv-hash">{parts.hash}</span><span class="drv-name">{parts.name}</span>
        </div>
        <div class="where" class:remote={whereIsRemote(build.host)} title={whereLabel(build.host)}>
          {whereLabel(build.host)}
        </div>
        <div class="phase">{build.phase ?? ''}</div>
        <div class="duration">{formatDuration(elapsedMs(build))}</div>
        <div class="right">{String(build.logCount)}</div>
      </div>
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
