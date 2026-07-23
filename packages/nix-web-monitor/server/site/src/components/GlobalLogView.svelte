<script lang="ts">
  /// Inline tail of one machine build's on-disk log, fetched from the server's
  /// `/api/global-log` endpoint (which decompresses the live `.drv.bz2` file).
  /// Polls while mounted: the panel mounts one of these per *expanded* row, so
  /// a collapsed log costs nothing.

  import { getPaneVisibility } from '$lib/panes/context';

  type Props = {
    /// Exact active worker generation whose log to tail. The server resolves
    /// this identity to the path recorded in the machine-wide build view.
    drvPath: string;
    pid: number;
    startTime: number;
    /// Server-sampled kernel start time (procfs ticks on Linux, the sysctl
    /// start timestamp on macOS): the true worker generation, which pins the
    /// tail to this worker even if the pid is recycled for the same drv
    /// within the same startTime second. Never null: a goal whose generation
    /// the server could not sample offers no log drawer at all (the rows gate
    /// on it), because a generation-less identity is exactly what a
    /// same-second pid recycle could silently retarget.
    startTicks: number;
  };

  const { drvPath, pid, startTime, startTicks }: Props = $props();

  /// Whether the pane hosting this drawer is currently shown. The poll keeps
  /// running while hidden (the drawer stays mounted), but the tail-follow
  /// scroll must wait for layout to exist -- see the follow effect below.
  const paneVisible = getPaneVisibility();

  /// Refetch cadence while open; matches the global probe's two-second poll,
  /// so the tail is as live as the row it belongs to.
  const POLL_MS = 2000;

  let text = $state('');
  let note = $state<string | null>('loading log…');
  let stream = $state<HTMLPreElement | null>(null);

  async function fetchTail(
    targetDrvPath: string,
    targetPid: number,
    targetStartTime: number,
    targetStartTicks: number
  ): Promise<void> {
    try {
      const query = new URLSearchParams({
        drv: targetDrvPath,
        pid: String(targetPid),
        start: String(targetStartTime),
        startTicks: String(targetStartTicks)
      });
      const response = await fetch(`/api/global-log?${query.toString()}`);
      if (!response.ok) {
        // Keep showing a stale tail over a placeholder: a 404 mid-build just
        // means the builder has not flushed (or the entry blinked); the next
        // poll usually recovers.
        if (text.length === 0) {
          note = response.status === 404 ? 'no log output yet' : 'log unavailable';
        }
        return;
      }
      const body = await response.text();
      if (body.length === 0) {
        if (text.length === 0) note = 'no log output yet';
        return;
      }
      text = body;
      note = null;
    } catch {
      if (text.length === 0) note = 'log unavailable';
    }
  }

  $effect(() => {
    // Fetch on mount / re-target, then poll. The interval dies with the
    // component, so collapsing the row stops the traffic. A tick that lands
    // while the previous fetch is still in flight is skipped: tailing a big
    // log (large `.drv.bz2`, slow disk) can outlast the poll period, and
    // stacking another expensive server read on top only makes it slower.
    let inFlight = false;
    const tick = (): void => {
      if (inFlight) return;
      inFlight = true;
      // `fetchTail` never rejects (it catches internally), so `finally` is
      // just the settle hook.
      void fetchTail(drvPath, pid, startTime, startTicks).finally(() => {
        inFlight = false;
      });
    };
    tick();
    const timer = setInterval(tick, POLL_MS);
    return () => {
      clearInterval(timer);
    };
  });

  $effect(() => {
    // Pin to the newest lines on every update: this is a tail view, not a
    // scrollback browser (the full log stays available via `nix log` later).
    void text;
    // Reading `paneVisible()` both skips the scroll while the hosting pane is
    // hidden -- with `display: none` the stream has no layout, so scrollHeight
    // is 0 and the write would record scrollTop 0 -- and re-runs this effect
    // when the pane is shown again, snapping to the live tail even if the
    // build quieted (no new `text`) while it was hidden.
    if (stream === null || !paneVisible()) return;
    stream.scrollTop = stream.scrollHeight;
  });
</script>

{#if note !== null}
  <div class="global-log-note">{note}</div>
{:else}
  <pre class="global-log-view" bind:this={stream}>{text}</pre>
{/if}
