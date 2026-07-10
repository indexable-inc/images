<script lang="ts">
  import PanelHeader from '$lib/PanelHeader.svelte';
  import type { DaemonInfo, DaemonOps } from '$lib/types';

  type Props = {
    daemon: DaemonInfo;
  };

  const { daemon }: Props = $props();

  /// Op classes in a fixed display order: the `DaemonOps` field (so the row
  /// count joins by identity), a short label, and a tooltip spelling out what
  /// the class counts. The syscall lists mirror `OpClass::classify` in
  /// `parser/src/daemon.rs`; update both together.
  const OP_ORDER: ReadonlyArray<readonly [keyof DaemonOps, string, string]> = [
    ['link', 'link', 'link, linkat, clonefile: hard-linking, dominates store optimisation'],
    ['rename', 'rename', 'rename, renameat: finished paths moving into place'],
    ['write', 'write', 'write, pwrite, writev: file data being written'],
    ['fsync', 'fsync', 'fsync, fdatasync: writes being flushed to disk'],
    ['open', 'open', 'open, openat'],
    ['stat', 'stat', 'stat, lstat, fstat, access, getattrlist, readlink: metadata reads'],
    ['unlink', 'unlink', 'unlink, rmdir: paths being deleted'],
    [
      'other',
      'other',
      'everything else: syscalls with no class of their own (ioctl, mmap, getdirentries, …) ' +
        'plus, on macOS, the disk-I/O rows fs_usage interleaves (RdData, WrData, PgIn, …)'
    ]
  ];

  const rows = $derived(
    OP_ORDER.map(([key, label, detail]) => ({ label, detail, count: daemon.ops[key] })).filter(
      (row) => row.count > 0
    )
  );
  const max = $derived(rows.reduce((peak, row) => Math.max(peak, row.count), 1));

  function pct(count: number): number {
    return Math.max(2, Math.round((count / max) * 100));
  }
</script>

<section class="panel daemon-panel">
  <PanelHeader title="daemon">
    {#if daemon.tracing}
      <span class="panel-meta"
        >{daemon.workers.length} worker{daemon.workers.length === 1 ? '' : 's'} &middot; {daemon.opsPerSec}/s</span
      >
    {/if}
  </PanelHeader>

  <div class="daemon-body">
    {#if !daemon.tracing}
      <!-- No tracer attached (no daemon, or it needs root). The status string
           explains why, so the panel never sits blank. -->
      <div class="daemon-status">{daemon.status || 'waiting for the daemon…'}</div>
    {:else if rows.length === 0}
      <div class="daemon-status">attached &middot; idle (no syscalls yet)</div>
    {:else}
      <div class="daemon-ops">
        {#each rows as row (row.label)}
          <div class="daemon-op" title="{String(row.count)} {row.label} &middot; {row.detail}">
            <span class="daemon-op-label">{row.label}</span>
            <span class="daemon-op-bar" aria-hidden="true"
              ><span class="daemon-op-fill" style="--p: {String(pct(row.count))}%"></span></span
            >
            <span class="daemon-op-count">{row.count}</span>
          </div>
        {/each}
      </div>
      {#if daemon.currentPath !== null}
        <!-- The most recent path any traced syscall touched: a "currently
             working on" readout, not tied to the op-class rows above. The
             &lrm; marks pin character order under the rtl ellipsis trick
             (see .daemon-path-value in style.css). -->
        <div class="daemon-path" title="most recently touched path: {daemon.currentPath}">
          <span class="daemon-path-label">touching</span>
          <span class="daemon-path-value">&lrm;{daemon.currentPath}&lrm;</span>
        </div>
      {/if}
      {#if daemon.hotPaths.length > 0}
        <div class="daemon-hot">
          <div
            class="daemon-hot-title"
            title="highest-traffic paths across all traced syscalls: rate this second, then total"
          >
            hot paths
          </div>
          {#each daemon.hotPaths as hot (hot.path)}
            <div class="daemon-hot-row" title={hot.path}>
              <span class="daemon-hot-path">&lrm;{hot.path}&lrm;</span>
              <span class="daemon-hot-rate">{hot.opsPerSec}/s</span>
              <span class="daemon-hot-count">{hot.count}</span>
            </div>
          {/each}
        </div>
      {/if}
    {/if}
  </div>
</section>
