<script lang="ts">
  import { formatDuration, formatRate } from '$lib/format';
  import { useNow } from '$lib/now.svelte';
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

  const now = useNow();

  type StatusCounts = Readonly<Record<BuildStatus, number>>;

  const counts = $derived(
    snapshot.builds.reduce<StatusCounts>(
      (acc, build) => ({ ...acc, [build.status]: acc[build.status] + 1 }),
      { planned: 0, running: 0, stopped: 0, succeeded: 0, failed: 0 }
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

  /// Overall run wall-clock: earliest activity start to last activity stop
  /// while running, frozen at the final span once the run finishes. The
  /// snapshot carries no finish timestamp, so the last observed stop is the
  /// best end marker; it falls back to the live clock only if nothing stopped.
  const startedAtMs = $derived(
    snapshot.activities.reduce<number | null>(
      (min, activity) => (min === null ? activity.startedAtMs : Math.min(min, activity.startedAtMs)),
      null
    )
  );
  const lastStopMs = $derived(
    snapshot.activities.reduce<number | null>(
      (max, activity) =>
        activity.stoppedAtMs === null
          ? max
          : max === null
            ? activity.stoppedAtMs
            : Math.max(max, activity.stoppedAtMs),
      null
    )
  );
  const elapsedMs = $derived.by(() => {
    if (startedAtMs === null) return null;
    const end = snapshot.finished ? (lastStopMs ?? now.value) : now.value;
    return Math.max(0, end - startedAtMs);
  });

  /// Running activities by kind. This is the breakdown that explains a run whose
  /// build rows look busy while the host CPU sits idle: the wall-clock is going
  /// into substituter transfers, store copies, and path-info queries, not
  /// compute. Nix's stream does not link an input download to the specific build
  /// that needs it, so this is the honest fleet-wide view rather than a per-build
  /// attribution.
  function runningOfType(name: string): number {
    return snapshot.activities.filter(
      (activity) => activity.activityType.name === name && activity.status === 'running'
    ).length;
  }

  const downloading = $derived(runningOfType('file_transfer'));
  const copying = $derived(runningOfType('copy_path') + runningOfType('copy_paths'));
  const querying = $derived(runningOfType('query_path_info'));

  /// Monotonic total of bytes pulled from substituters: each file-transfer
  /// activity's `done` counter summed. Stopped transfers keep their final value,
  /// so the sum only grows and a time delta yields a live rate.
  const downloadedBytes = $derived(
    snapshot.activities.reduce(
      (sum, activity) =>
        activity.activityType.name === 'file_transfer' ? sum + (activity.progress?.done ?? 0) : sum,
      0
    )
  );

  /// Per-second samples of `downloadedBytes`, held off the reactive graph so the
  /// effect that writes `downloadRate` never depends on its own output. The tick
  /// gate means snapshot bursts within one second do not each compute a rate.
  const sample = { bytes: 0, at: 0, rate: 0, seeded: false };
  let downloadRate = $state(0);
  $effect(() => {
    const at = now.value;
    const bytes = downloadedBytes;
    if (sample.seeded && at > sample.at) {
      const perSecond = Math.max(0, ((bytes - sample.bytes) / (at - sample.at)) * 1000);
      // Light EWMA so the reading does not jump between snapshot bursts.
      sample.rate = sample.rate * 0.4 + perSecond * 0.6;
      downloadRate = sample.rate;
    }
    sample.bytes = bytes;
    sample.at = at;
    sample.seeded = true;
  });
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

    {#if elapsedMs !== null}
      <div class="kpi" class:kpi-good={snapshot.finished && exit === 0}>
        <span class="kpi-num">{formatDuration(elapsedMs)}</span>
        <span class="kpi-label">{snapshot.finished ? 'total' : 'elapsed'}</span>
      </div>
    {/if}

    {#if counts.planned > 0}
      <div class="kpi">
        <span class="kpi-num">{counts.planned}</span>
        <span class="kpi-label">queued</span>
      </div>
    {/if}
    {#if counts.running > 0}
      <div class="kpi kpi-warn">
        <span class="kpi-num">{counts.running}</span>
        <span class="kpi-label">running</span>
      </div>
    {/if}
    {#if downloading > 0}
      <div class="kpi kpi-info">
        <span class="kpi-num">{downloading}</span>
        <span class="kpi-label">downloading</span>
        {#if downloadRate > 0}
          <span class="kpi-num kpi-faint">{formatRate(downloadRate)}</span>
        {/if}
      </div>
    {/if}
    {#if copying > 0}
      <div class="kpi kpi-info">
        <span class="kpi-num">{copying}</span>
        <span class="kpi-label">copying</span>
      </div>
    {/if}
    {#if querying > 0}
      <div class="kpi kpi-info">
        <span class="kpi-num">{querying}</span>
        <span class="kpi-label">querying</span>
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
