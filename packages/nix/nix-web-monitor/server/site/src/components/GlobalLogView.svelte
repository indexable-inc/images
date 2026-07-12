<script lang="ts">
  /// Inline tail of one machine build's on-disk log, fetched from the server's
  /// `/api/global-log` endpoint (which decompresses the live `.drv.bz2` file).
  /// Polls while mounted: the panel mounts one of these per *expanded* row, so
  /// a collapsed log costs nothing.

  type Props = {
    /// Exact active worker generation whose log to tail. The server resolves
    /// this identity to the path recorded in the machine-wide build view.
    drvPath: string;
    pid: number;
    startTime: number;
  };

  const { drvPath, pid, startTime }: Props = $props();

  /// Refetch cadence while open; matches the global probe's two-second poll,
  /// so the tail is as live as the row it belongs to.
  const POLL_MS = 2000;

  let text = $state('');
  let note = $state<string | null>('loading log…');
  let stream = $state<HTMLPreElement | null>(null);

  async function fetchTail(
    targetDrvPath: string,
    targetPid: number,
    targetStartTime: number
  ): Promise<void> {
    try {
      const query = new URLSearchParams({
        drv: targetDrvPath,
        pid: String(targetPid),
        start: String(targetStartTime)
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
    // component, so collapsing the row stops the traffic.
    void fetchTail(drvPath, pid, startTime);
    const timer = setInterval(() => void fetchTail(drvPath, pid, startTime), POLL_MS);
    return () => {
      clearInterval(timer);
    };
  });

  $effect(() => {
    // Pin to the newest lines on every update: this is a tail view, not a
    // scrollback browser (the full log stays available via `nix log` later).
    void text;
    if (stream !== null) stream.scrollTop = stream.scrollHeight;
  });
</script>

{#if note !== null}
  <div class="global-log-note">{note}</div>
{:else}
  <pre class="global-log-view" bind:this={stream}>{text}</pre>
{/if}
