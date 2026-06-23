<script lang="ts">
  import PanelHeader from '$lib/PanelHeader.svelte';
  import { middleTruncate } from '$lib/format';
  import type { DaemonInfo, DaemonOps } from '$lib/types';

  type Props = {
    daemon: DaemonInfo;
  };

  const { daemon }: Props = $props();

  /// Op classes in a fixed display order with a short label. Keyed by the
  /// `DaemonOps` field so the row count joins by identity.
  const OP_ORDER: ReadonlyArray<readonly [keyof DaemonOps, string]> = [
    ['link', 'link'],
    ['rename', 'rename'],
    ['write', 'write'],
    ['fsync', 'fsync'],
    ['open', 'open'],
    ['stat', 'stat'],
    ['unlink', 'unlink'],
    ['other', 'other']
  ];

  const rows = $derived(
    OP_ORDER.map(([key, label]) => ({ label, count: daemon.ops[key] })).filter(
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
          <div class="daemon-op" title="{String(row.count)} {row.label}">
            <span class="daemon-op-label">{row.label}</span>
            <span class="daemon-op-bar" aria-hidden="true"
              ><span class="daemon-op-fill" style="--p: {String(pct(row.count))}%"></span></span
            >
            <span class="daemon-op-count">{row.count}</span>
          </div>
        {/each}
      </div>
      {#if daemon.currentPath !== null}
        <div class="daemon-path" title={daemon.currentPath}>
          {middleTruncate(daemon.currentPath, 56)}
        </div>
      {/if}
      {#if daemon.hotPaths.length > 0}
        <div class="daemon-hot">
          <div class="daemon-hot-title">hot paths</div>
          {#each daemon.hotPaths as hot (hot.path)}
            <div class="daemon-hot-row" title={hot.path}>
              <span class="daemon-hot-path">{middleTruncate(hot.path, 48)}</span>
              <span class="daemon-hot-rate">{hot.opsPerSec}/s</span>
              <span class="daemon-hot-count">{hot.count}</span>
            </div>
          {/each}
        </div>
      {/if}
    {/if}
  </div>
</section>
