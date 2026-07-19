<script lang="ts">
  import type { BenchCell, BenchColumn, BenchRow, BenchSource } from './bench';

  const {
    columns,
    rows,
    resistanceMax = 12
  }: {
    columns: readonly BenchColumn[];
    rows: BenchRow[];
    resistanceMax?: number;
  } = $props();

  const sourceMark: Record<BenchSource, string> = {
    vendor: 'v',
    third: '3p',
    card: 'sc'
  };

  type Hover = { rowId: string; rowLabel: string; colKey: string; text: string } | null;
  let hovered = $state<Hover>(null);

  function hoverText(row: BenchRow, cell: BenchCell | undefined): string {
    const parts: string[] = [];
    if (cell !== undefined) {
      const origin =
        cell.source === 'vendor'
          ? 'vendor self-reported'
          : cell.source === 'third'
            ? 'third party ran the model'
            : 'local system card';
      parts.push(`${cell.value} (${origin})`);
      if (cell.note !== undefined) parts.push(cell.note);
    } else {
      parts.push('no comparable published number; left blank rather than guessed');
    }
    if (row.note !== undefined) parts.push(row.note);
    return parts.join('. ');
  }
</script>

<figure class="bench">
  <div class="scroll">
    <table>
      <thead>
        <tr>
          <th class="name"></th>
          {#each columns as col (col.key)}
            <th class="model">{col.label}</th>
          {/each}
          <th class="resistance" title="gaming-resistance rubric total from the matrix above">rubric</th>
        </tr>
      </thead>
      <tbody onpointerleave={() => (hovered = null)}>
        {#each rows as row (row.id)}
          <tr>
            <th class="name" scope="row">
              {#if row.href !== undefined}
                <!-- Callers pass external hrefs; a generic component cannot resolve. -->
                <!-- eslint-disable-next-line svelte/no-navigation-without-resolve -->
                <a href={row.href}>{row.label}</a>
              {:else}
                {row.label}
              {/if}
            </th>
            {#each columns as col (col.key)}
              {@const cell = row.cells[col.key]}
              <td
                class="cell"
                class:empty={cell === undefined}
                class:lit={hovered !== null && hovered.rowId === row.id && hovered.colKey === col.key}
                onpointerenter={() =>
                  (hovered = { rowId: row.id, rowLabel: row.label, colKey: col.key, text: hoverText(row, cell) })}
              >
                {#if cell !== undefined}
                  {cell.value}<sup>{sourceMark[cell.source]}</sup>
                {:else}
                  -
                {/if}
              </td>
            {/each}
            <td class="resistance">
              {#if row.resistance !== undefined}
                {row.resistance}/{resistanceMax}
              {:else}
                -
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  <figcaption class="status">
    {#if hovered}
      <strong>{hovered.rowLabel}</strong>: {hovered.text}
    {:else}
      Hover a cell for provenance and harness notes.
      Marks: <sup>v</sup> vendor self-reported, <sup>3p</sup> third party ran the model, <sup>sc</sup> local system card.
    {/if}
  </figcaption>
</figure>

<style>
  .bench {
    margin: var(--cell-h) 0 calc(var(--cell-h) * 2);
  }

  .scroll {
    max-width: 100%;
    overflow-x: auto;
    border: 1px solid var(--rule);
    border-radius: var(--radius);
  }

  table {
    border-collapse: collapse;
    width: 100%;
  }

  th,
  td {
    padding: 0 1ch;
    height: var(--cell-h);
    white-space: nowrap;
  }

  thead th {
    color: var(--fg-muted);
    font-weight: 400;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    border-bottom: 1px solid var(--rule);
  }

  th.name {
    text-align: left;
    font-weight: 400;
  }

  th.model,
  td.cell {
    text-align: right;
  }

  th.resistance,
  td.resistance {
    text-align: right;
    color: var(--fg-muted);
    font-family: var(--font-mono);
  }

  thead th.resistance {
    cursor: help;
  }

  tbody tr + tr td,
  tbody tr + tr th {
    border-top: 1px solid var(--rule);
  }

  .cell {
    font-family: var(--font-mono);
  }

  .cell sup {
    color: var(--fg-faint);
    margin-left: 0.5ch;
  }

  .cell.empty {
    color: var(--fg-faint);
  }

  .cell.lit {
    background: var(--code);
  }

  .status {
    min-height: calc(var(--cell-h) * 2);
    color: var(--fg-muted);
    padding-top: calc(var(--cell-h) / 3);
  }
</style>
