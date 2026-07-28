<script lang="ts">
  import Box from './Box.svelte';
  import {
    bar,
    fmtUsdPerSecond,
    maxTotalUsd,
    resourceRows,
    totalBarFraction
  } from './resource-display';
  import type { Status, UsageStats } from './stats';

  type Props = {
    stats: UsageStats;
    status: Status;
  };

  const { stats, status }: Props = $props();

  const rows = $derived(resourceRows(stats));
  const total = $derived(stats.costPerSecondUsd);
  const totalFrac = $derived(totalBarFraction(total, maxTotalUsd(stats)));
</script>

<Box title="agent-vm" collapsible={false}>
  {#snippet hintSnippet()}
    <span class="status" data-state={status}><span class="dot" aria-hidden="true">●</span> {status}</span>
  {/snippet}

  <div class="grid">
    {#each rows as r (r.key)}
      <span class="k" class:hot={r.fraction > 0.01}>{r.label}</span>
      <span
        class="bar"
        role="meter"
        aria-label={r.ariaLabel}
        aria-valuemin="0"
        aria-valuemax="1"
        aria-valuenow={r.fraction}
        aria-valuetext={r.display}
      >{bar(r.fraction)}</span>
      <span class="v">{r.display}</span>
    {/each}
  </div>

  <hr />

  <div class="breakdown">
    {#each rows as r (r.key)}
      <span class="br">{r.rateLabel}</span>
      <span class="bx">&times;</span>
      <span class="bv">{r.breakdownValue}</span>
      <span class="bt">{fmtUsdPerSecond(r.bill)}</span>
    {/each}
  </div>

  <div class="total-rule"></div>
  <div class="total-row">
    <span class="total-label">TOTAL</span>
    <span class="total-bar">{bar(totalFrac)}</span>
    <span class="total-val">{fmtUsdPerSecond(total)}</span>
  </div>
</Box>

<style>
  .grid {
    display: grid;
    grid-template-columns: 8ch auto 1fr;
    column-gap: 2ch;
    row-gap: 0.1lh;
    align-items: baseline;
  }

  .k {
    color: var(--ix-ink-faint);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    transition: color 0.2s;
  }

  .k.hot {
    color: var(--ix-ink-strong);
  }

  .bar {
    color: var(--ix-ink);
    letter-spacing: -0.05em;
    white-space: nowrap;
    overflow: hidden;
  }

  .v {
    color: var(--ix-ink-muted);
    text-align: right;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  hr {
    border: none;
    border-top: 1px dashed var(--ix-border);
    margin: 0;
    opacity: 0.7;
  }

  .breakdown {
    display: grid;
    grid-template-columns: auto auto 1fr auto;
    column-gap: 1ch;
    row-gap: 0.1lh;
  }

  .br {
    color: var(--ix-ink-muted);
    font-variant-numeric: tabular-nums;
    text-align: right;
    white-space: nowrap;
  }

  .bx {
    color: var(--ix-ink-faint);
  }

  .bv {
    color: var(--ix-ink-muted);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .bt {
    color: var(--ix-ink-strong);
    font-variant-numeric: tabular-nums;
    text-align: right;
    white-space: nowrap;
  }

  .total-rule {
    border-top: 1px solid var(--ix-ink-faint);
  }

  .total-row {
    display: grid;
    grid-template-columns: 8ch auto 1fr;
    column-gap: 2ch;
    align-items: baseline;
  }

  .total-label {
    color: var(--ix-ink-strong);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .total-bar {
    color: var(--ix-ink-strong);
    letter-spacing: -0.05em;
    white-space: nowrap;
  }

  .total-val {
    color: var(--ix-ink-strong);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    text-align: right;
  }

  .status .dot {
    color: var(--ix-ink-faint);
  }

  .status[data-state='live'] {
    color: var(--ix-ink-strong);
  }

  .status[data-state='live'] .dot {
    color: var(--ix-ink-strong);
  }

  .status[data-state='waiting'] .dot {
    color: var(--ix-ink-muted);
  }

  @media (max-width: 520px) {
    .grid,
    .total-row {
      grid-template-columns: 5ch auto 1fr;
      column-gap: 1ch;
    }

    .breakdown {
      grid-template-columns: auto auto 1fr auto;
      column-gap: 0.75ch;
    }
  }
</style>
