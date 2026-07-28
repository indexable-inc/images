<script lang="ts">
  import type { BenchColumn, BenchRow } from './bench';

  const {
    columns,
    rows,
    resistanceMax = 12
  }: {
    columns: readonly BenchColumn[];
    rows: BenchRow[];
    resistanceMax?: number;
  } = $props();

  // One panel per benchmark that has at least two plottable scores,
  // ordered by gaming resistance so the trustworthy comparisons lead.
  const panels = $derived(
    [...rows]
      .filter((row) => columns.filter((col) => row.cells[col.key]?.chart !== undefined).length >= 2)
      .sort((a, b) => (b.resistance ?? -1) - (a.resistance ?? -1))
  );
</script>

<figure class="panels">
  <div class="grid">
    {#each panels as row (row.id)}
      <section class="panel">
        <h4>
          <span class="bench-label">{row.label}</span>
          {#if row.resistance !== undefined}
            <span class="rubric">{row.resistance}/{resistanceMax}</span>
          {/if}
        </h4>
        {#each columns as col (col.key)}
          {@const cell = row.cells[col.key]}
          <div class="bar-row">
            <span class="model">{col.label}</span>
            {#if cell?.chart !== undefined}
              <span class="track">
                <span
                  class="bar"
                  class:self={cell.source !== 'third'}
                  style:width="{cell.chart}%"
                  title={cell.note}
                ></span>
              </span>
              <span class="value">{cell.value}</span>
            {:else}
              <span class="track none"></span>
              <span class="value none">none</span>
            {/if}
          </div>
        {/each}
      </section>
    {/each}
  </div>
  <figcaption>
    All axes run 0 to 100. Solid: a third party ran the model. Hatched:
    self-reported by the vendor or its system card. Empty track: no
    published number for that model.
  </figcaption>
</figure>

<style>
  .panels {
    margin: var(--cell-h) 0 calc(var(--cell-h) * 2);
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(38ch, 1fr));
    gap: var(--cell-h) 4ch;
  }

  h4 {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    margin: 0 0 calc(var(--cell-h) / 3);
    font-weight: 400;
  }

  .bench-label {
    color: var(--fg-strong);
  }

  .rubric {
    color: var(--fg-faint);
    font-family: var(--font-mono);
    cursor: help;
  }

  .bar-row {
    display: grid;
    grid-template-columns: 13ch 1fr 12ch;
    align-items: center;
    height: var(--cell-h);
    gap: 0 1ch;
  }

  .model {
    color: var(--fg-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .track {
    background: var(--code);
    border-radius: 1px;
    height: calc(var(--cell-h) / 2);
  }

  .track.none {
    background: transparent;
    border: 1px dashed var(--rule);
  }

  .bar {
    display: block;
    height: 100%;
    min-width: 2px;
    border-radius: 1px;
    background: var(--fg-muted);
  }

  .bar.self {
    background: repeating-linear-gradient(
      -45deg,
      var(--fg-faint) 0 3px,
      transparent 3px 6px
    );
    box-shadow: inset 0 0 0 1px var(--fg-faint);
  }

  .value {
    font-family: var(--font-mono);
    text-align: right;
    white-space: nowrap;
  }

  .value.none {
    color: var(--fg-faint);
  }

  figcaption {
    color: var(--fg-muted);
    padding-top: calc(var(--cell-h) / 3);
  }
</style>
