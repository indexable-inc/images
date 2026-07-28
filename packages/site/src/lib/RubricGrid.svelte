<script lang="ts">
  import {
    rubricCellOf,
    rubricTotal,
    validateRubric,
    type RubricDimension,
    type RubricItem
  } from './rubric';

  const {
    items,
    dimensions,
    maxCell = 2
  }: {
    items: RubricItem[];
    dimensions: readonly RubricDimension[];
    maxCell?: number;
  } = $props();

  const maxTotal = $derived(dimensions.length * maxCell);
  const ticks = $derived(Array.from({ length: maxTotal }, (_, i) => i));
  const ranked = $derived.by(() => {
    validateRubric(dimensions, items, maxCell);
    return [...items].sort((a, b) => rubricTotal(dimensions, b) - rubricTotal(dimensions, a));
  });

  type Hover = { item: RubricItem; dim: RubricDimension } | null;
  let hovered = $state<Hover>(null);
</script>

<figure class="rubric">
  <div class="scroll">
    <table>
      <thead>
        <tr>
          <th class="name"></th>
          {#each dimensions as dim (dim.key)}
            <th class="dim" title={dim.meaning}>{dim.label}</th>
          {/each}
          <th class="total">total</th>
        </tr>
      </thead>
      <tbody onpointerleave={() => (hovered = null)}>
        {#each ranked as item (item.id)}
          {@const total = rubricTotal(dimensions, item)}
          <tr>
            <th class="name" scope="row">
              {#if item.href !== undefined}
                <!-- Callers pass external hrefs; a generic component cannot resolve. -->
                <!-- eslint-disable-next-line svelte/no-navigation-without-resolve -->
                <a href={item.href}>{item.label}</a>
              {:else}
                {item.label}
              {/if}
            </th>
            {#each dimensions as dim (dim.key)}
              {@const cell = rubricCellOf(item.id, item.cells, dim.key)}
              <td
                class="cell"
                class:zero={cell.value === 0}
                class:full={cell.value === maxCell}
                class:lit={hovered !== null && hovered.item === item && hovered.dim === dim}
                onpointerenter={() => (hovered = { item, dim })}
              >
                {cell.value}
              </td>
            {/each}
            <td class="total">
              <span class="meter" aria-hidden="true">
                {#each ticks as i (i)}
                  <span class="tick" class:on={i < total}></span>
                {/each}
              </span>
              {total}/{maxTotal}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  <figcaption class="status">
    {#if hovered}
      {@const cell = rubricCellOf(hovered.item.id, hovered.item.cells, hovered.dim.key)}
      <strong>{hovered.item.label}</strong> / {hovered.dim.label}
      {cell.value}/{maxCell}: {cell.why}
    {:else}
      Hover a cell for the score justification. Column meanings appear on the headers.
    {/if}
  </figcaption>
</figure>

<style>
  .rubric {
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
    text-align: center;
  }

  thead th {
    color: var(--fg-muted);
    font-weight: 400;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    border-bottom: 1px solid var(--rule);
    cursor: help;
  }

  th.name {
    text-align: left;
    font-weight: 400;
  }

  tbody tr + tr td,
  tbody tr + tr th {
    border-top: 1px solid var(--rule);
  }

  .cell {
    color: var(--fg-muted);
    font-family: var(--font-mono);
  }

  .cell.zero {
    color: var(--status-rejected);
  }

  .cell.full {
    color: var(--status-load-bearing);
  }

  .cell.lit {
    background: var(--code);
    color: var(--fg);
  }

  td.total {
    text-align: right;
    font-family: var(--font-mono);
  }

  .meter {
    display: inline-flex;
    gap: 1px;
    margin-right: 1ch;
    vertical-align: middle;
  }

  .tick {
    width: 4px;
    height: 9px;
    background: var(--rule);
  }

  .tick.on {
    background: var(--status-accepted);
  }

  .status {
    min-height: calc(var(--cell-h) * 2);
    color: var(--fg-muted);
    padding-top: calc(var(--cell-h) / 3);
  }
</style>
