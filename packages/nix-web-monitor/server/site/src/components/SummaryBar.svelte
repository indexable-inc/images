<script lang="ts">
  import {
    ACTIVITY_NAME_BUILD,
    type BuildStatus,
    type ConnectionStatus,
    type MonitorSnapshot
  } from '$lib/types';

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

  const progressPercent = $derived(
    snapshot.progress === null || snapshot.progress.expected <= 0
      ? null
      : Math.min(100, Math.round((snapshot.progress.done / snapshot.progress.expected) * 100))
  );

  const exit = $derived(snapshot.exitCode);
</script>

<header class="summary">
  <div class="brand">nix-web-monitor</div>

  <div class="kpi-group">
    <div class="kpi" data-status={status}>
      <span class="kpi-dot"></span>
      <span class="kpi-label">{status}</span>
    </div>

    <div class="kpi">
      <span class="kpi-num">{snapshot.builds.length}</span>
      <span class="kpi-sep">/</span>
      <span class="kpi-num kpi-faint">{expectedBuilds}</span>
      <span class="kpi-label">builds</span>
    </div>

    {#if counts.running > 0}
      <div class="kpi kpi-warn">
        <span class="kpi-num">{counts.running}</span>
        <span class="kpi-label">running</span>
      </div>
    {/if}
    {#if counts.succeeded > 0}
      <div class="kpi kpi-good">
        <span class="kpi-num">{counts.succeeded}</span>
        <span class="kpi-label">ok</span>
      </div>
    {/if}
    {#if counts.stopped > 0}
      <div class="kpi kpi-info">
        <span class="kpi-num">{counts.stopped}</span>
        <span class="kpi-label">done</span>
      </div>
    {/if}
    {#if counts.failed > 0}
      <div class="kpi kpi-bad">
        <span class="kpi-num">{counts.failed}</span>
        <span class="kpi-label">failed</span>
      </div>
    {/if}

    {#if snapshot.errors.length > 0}
      <div class="kpi kpi-bad">
        <span class="kpi-num">{snapshot.errors.length}</span>
        <span class="kpi-label">errors</span>
      </div>
    {/if}

    {#if exit !== null}
      <div class="kpi" class:kpi-good={exit === 0} class:kpi-bad={exit !== 0}>
        <span class="kpi-label">exit</span>
        <span class="kpi-num">{exit}</span>
      </div>
    {/if}
  </div>

  {#if progressPercent !== null}
    <div class="progress" title="{String(snapshot.progress?.done ?? 0)} / {String(snapshot.progress?.expected ?? 0)}">
      <div class="progress-bar" style="--progress: {String(progressPercent)}%"></div>
      <span class="progress-text">{String(progressPercent)}%</span>
    </div>
  {/if}
</header>
