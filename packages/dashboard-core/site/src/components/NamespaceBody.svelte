<script lang="ts">
  import type { Pane } from '$lib/types';

  // The namespace renderer: a `data` pane whose body is a JSON array of variable
  // rows produced by the kernel — one Python session's live globals. Each row is
  // {name, type, kind, repr, size, shape}; we render them as a compact table, the
  // heaviest first (the producer sorts), so the eye lands on what holds the memory.
  type Row = {
    name: string;
    type: string;
    kind: string;
    repr: string;
    size: number;
    shape: string;
  };

  let { pane }: { pane: Pane } = $props();

  const rows = $derived.by<Row[]>(() => {
    try {
      const parsed = JSON.parse(pane.body ?? '[]');
      return Array.isArray(parsed) ? parsed : [];
    } catch {
      return [];
    }
  });

  // A short chip per kind so the lead column stays narrow and scannable.
  const CHIP: Record<string, string> = {
    module: 'mod',
    class: 'cls',
    function: 'fn',
    scalar: 'num',
    text: 'str',
    sequence: 'seq',
    mapping: 'map',
    array: 'arr',
    frame: 'df',
    object: 'obj',
  };
  function chip(kind: string): string {
    return CHIP[kind] ?? kind.slice(0, 3);
  }

  // Human byte size; empty for the sizeless (modules, functions report 0).
  function fmtSize(n: number): string {
    if (!n) return '';
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`;
    if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
    return `${(n / (1024 * 1024 * 1024)).toFixed(1)} GB`;
  }

  // The middle column: a frame/array describes itself by shape; everything else
  // shows its repr, falling back to a shape (a container's length).
  function detail(row: Row): string {
    if (row.shape && (row.kind === 'frame' || row.kind === 'array')) return row.shape;
    return row.repr || row.shape;
  }
</script>

<div class="ns">
  {#if rows.length === 0}
    <div class="ns-empty">no variables</div>
  {:else}
    <table class="ns-table">
      <tbody>
        {#each rows as row (row.name)}
          <tr>
            <td class="ns-kind">
              <span class="ns-chip" data-kind={row.kind}>{chip(row.kind)}</span>
            </td>
            <td class="ns-name" title={row.type}>{row.name}</td>
            <td class="ns-detail" title={detail(row)}>{detail(row)}</td>
            <td class="ns-size">{fmtSize(row.size)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .ns {
    padding: 4px 0;
    font-family: var(--mono);
    font-size: 12px;
    line-height: 1.5;
  }
  .ns-empty {
    padding: 8px 12px;
    color: var(--ink-faint);
    font-style: italic;
  }
  .ns-table {
    width: 100%;
    border-collapse: collapse;
  }
  .ns-table td {
    padding: 5px 12px;
    vertical-align: baseline;
    border-bottom: 1px solid var(--edge);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .ns-table tr:last-child td {
    border-bottom: 0;
  }
  /* A quiet, square chip — flat like the rest of the canvas. */
  .ns-kind {
    width: 1%;
  }
  .ns-chip {
    display: inline-block;
    min-width: 2.6em;
    text-align: center;
    font-size: 10px;
    letter-spacing: 0.02em;
    color: var(--ink-faint);
    border: 1px solid var(--edge-strong);
    padding: 0 4px;
  }
  /* Frames and arrays carry the data weight; tint them with the accent. */
  .ns-chip[data-kind='frame'],
  .ns-chip[data-kind='array'] {
    color: var(--accent);
    border-color: var(--accent);
  }
  .ns-name {
    width: 1%;
    color: var(--ink);
  }
  .ns-detail {
    width: 100%;
    max-width: 0;
    color: var(--ink-faint);
  }
  .ns-size {
    width: 1%;
    text-align: right;
    color: var(--ink-dim);
    font-variant-numeric: tabular-nums;
  }
</style>
