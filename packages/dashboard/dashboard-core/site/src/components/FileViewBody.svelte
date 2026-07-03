<script lang="ts">
  import { highlightLines } from '$lib/highlight';
  import type { Pane } from '$lib/types';

  // The `file-view` data renderer: a read's structured card — file icon, path,
  // span meta, then the requested line slice with a real line-number gutter.
  // The producer ships the WHOLE file as highlight context when it fits (see
  // runtime.__ix_read), so a mid-file slice still tokenizes correctly; only the
  // start..end lines are shown.
  let { pane }: { pane: Pane } = $props();

  interface FileView {
    label?: string;
    file?: boolean;
    lang?: string | null;
    text?: string;
    context_start?: number;
    start?: number;
    end?: number;
    total?: number | null;
    chars?: number;
  }

  const view = $derived.by<FileView>(() => {
    try {
      const parsed: unknown = JSON.parse(pane.body ?? '');
      return parsed && typeof parsed === 'object' ? (parsed as FileView) : {};
    } catch {
      return {};
    }
  });

  const text = $derived(view.text ?? '');
  const rawLines = $derived(text.split('\n'));
  const contextStart = $derived(view.context_start ?? 1);
  const start = $derived(view.start ?? contextStart);
  const end = $derived(view.end ?? contextStart + rawLines.length - 1);
  // 0-based slice bounds into the shipped context, clamped: the display copy may
  // be line-clipped below the claimed span for a huge read.
  const from = $derived(Math.max(start - contextStart, 0));
  const to = $derived(Math.max(from, Math.min(end - contextStart + 1, rawLines.length)));
  const shown = $derived(rawLines.slice(from, to));

  const meta = $derived.by(() => {
    const chars = (view.chars ?? text.length).toLocaleString();
    if (view.total != null && start === 1 && end === view.total)
      return `${view.total} lines · ${chars} chars`;
    const of = view.total != null ? ` of ${view.total}` : '';
    return `lines ${start}–${end}${of} · ${chars} chars`;
  });

  // Ribbon color by extension for the file icon. Presentation-only, so the map
  // lives with the renderer, not on the wire.
  const EXT_COLORS: Record<string, string> = {
    py: '#3776ab', rs: '#dea584', go: '#00add8',
    js: '#f1e05a', mjs: '#f1e05a', cjs: '#f1e05a',
    ts: '#3178c6', tsx: '#3178c6', jsx: '#f1e05a',
    json: '#cbcb41', jsonl: '#cbcb41', ndjson: '#cbcb41',
    toml: '#9c4221', yaml: '#cb171e', yml: '#cb171e',
    ini: '#8a8a92', cfg: '#8a8a92', conf: '#8a8a92', env: '#8a8a92',
    nix: '#7e7eff',
    md: '#519aba', rst: '#519aba', txt: '#9aa0a6',
    sh: '#89e051', bash: '#89e051', zsh: '#89e051', fish: '#89e051', nu: '#3aa675',
    html: '#e44d26', htm: '#e44d26', xml: '#e37933',
    css: '#563d7c', scss: '#c6538c',
    csv: '#41b883', tsv: '#41b883', parquet: '#41b883',
    log: '#9aa0a6', lock: '#e3c15b', sql: '#dad8d8', pdf: '#e02d2d',
    png: '#a074c4', jpg: '#a074c4', jpeg: '#a074c4',
    gif: '#a074c4', svg: '#ffb13b', webp: '#a074c4',
  };
  const name = $derived((view.label ?? '').split('/').pop() ?? '');
  const ext = $derived.by(() => {
    const dot = name.lastIndexOf('.');
    const raw = dot > 0 ? name.slice(dot + 1) : name;
    return raw.toLowerCase().slice(0, 4) || 'txt';
  });
  const ribbon = $derived(EXT_COLORS[ext] ?? '#8a8a92');

  // Per-line highlighted HTML for the whole context (null until the highlighter
  // loads; raw text shows meanwhile and upgrades in place).
  let lineHtml = $state<string[] | null>(null);
  $effect(() => {
    const src = text;
    const l = view.lang ?? 'text';
    let alive = true;
    lineHtml = null;
    void highlightLines(src, l).then((out) => {
      if (alive) lineHtml = out;
    });
    return () => {
      alive = false;
    };
  });
</script>

<div class="fv">
  <header class="fv-head">
    {#if view.file !== false}
      <svg class="fv-icon" viewBox="0 0 40 50" fill="none" xmlns="http://www.w3.org/2000/svg">
        <path
          d="M5 2h21l9 9v35a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2z"
          class="fv-doc"
        />
        <path d="M26 2l9 9h-9z" class="fv-fold" />
        <rect x="3" y="30" width="34" height="14" rx="2" fill={ribbon} />
        <text x="20" y="40.5" class="fv-ext" text-anchor="middle">{ext.toUpperCase()}</text>
      </svg>
    {:else}
      <svg class="fv-icon" viewBox="0 0 40 50" fill="none" xmlns="http://www.w3.org/2000/svg">
        <rect x="3" y="6" width="34" height="38" rx="4" class="fv-doc" />
        <text x="20" y="33" class="fv-braces" text-anchor="middle">{'{ }'}</text>
      </svg>
    {/if}
    <div class="fv-id">
      <strong class="fv-label">{view.label || '(text)'}</strong>
      <span class="fv-meta">{meta}</span>
    </div>
  </header>
  <div class="fv-code">
    {#each shown as line, i (from + i)}
      <div class="fv-row">
        <span class="fv-ln">{start + i}</span>
        <span class="fv-lc">
          {#if lineHtml && lineHtml[from + i] !== undefined}
            <!-- shiki escapes token text, so injecting one highlighted line is safe. -->
            <!-- eslint-disable-next-line svelte/no-at-html-tags -->
            {@html lineHtml[from + i]}
          {:else}<span class="fv-raw">{line || ' '}</span>{/if}
        </span>
      </div>
    {/each}
  </div>
</div>

<style>
  .fv {
    display: flex;
    flex-direction: column;
    font-family: var(--mono);
  }
  .fv-head {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--edge);
  }
  .fv-icon {
    width: 15px;
    height: 19px;
    flex: none;
  }
  .fv-doc {
    fill: light-dark(#f2f2f4, #23232a);
    stroke: light-dark(#c9c9d2, #3a3a42);
    stroke-width: 1.5;
  }
  .fv-fold {
    fill: light-dark(#c9c9d2, #3a3a42);
  }
  .fv-ext {
    font-family: var(--mono);
    font-size: 11px;
    font-weight: 700;
    fill: #111;
  }
  .fv-braces {
    font-family: var(--mono);
    font-size: 18px;
    font-weight: 700;
    fill: var(--ink-faint);
  }
  .fv-id {
    min-width: 0;
  }
  .fv-label {
    display: block;
    font-size: 12px;
    font-weight: 600;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .fv-meta {
    display: block;
    margin-top: 2px;
    font-size: 10.5px;
    color: var(--ink-faint);
  }
  .fv-code {
    overflow: auto;
    max-height: 60vh;
    padding: 8px 0;
  }
  .fv-row {
    display: flex;
    align-items: baseline;
    line-height: 1.45;
    font-size: 11.5px;
    white-space: pre;
  }
  .fv-ln {
    flex: none;
    min-width: 3.5ch;
    padding: 0 10px 0 12px;
    text-align: right;
    color: var(--ink-faint);
    font-variant-numeric: tabular-nums;
    user-select: none;
  }
  .fv-lc {
    flex: 1 1 auto;
    min-width: 0;
    padding-right: 12px;
  }
  .fv-raw {
    color: var(--ink);
  }
  /* shiki tokens (injected as bare `.line` spans, same pattern as the inline
     trace): each carries both palettes as CSS vars; pick per OS scheme. */
  .fv-lc :global(.line),
  .fv-lc :global(.line span) {
    color: var(--shiki-light);
  }
  @media (prefers-color-scheme: dark) {
    .fv-lc :global(.line),
    .fv-lc :global(.line span) {
      color: var(--shiki-dark);
    }
  }
</style>
