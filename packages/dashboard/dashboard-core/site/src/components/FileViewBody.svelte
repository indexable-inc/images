<script lang="ts">
  import { highlightLines } from '$lib/highlight';
  import { fileIcon, valueIcon } from '$lib/icons';
  import palette from '$islands-theme';
  import type { Pane } from '$lib/types';

  // The `file-view` data renderer, styled like an editor pane (Zed/IntelliJ):
  // one slim header line — language icon, filename, dimmed directory, a tiny
  // right-aligned span — over the code slice on the islands editor background.
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
    truncated?: boolean;
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
  // String-valued deriveds so the highlight effect below value-compares (===)
  // and skips re-runs: `view` itself is a fresh object every SSE frame (the
  // pane store is reassigned per frame), and subscribing the effect to it
  // would re-tokenize the whole context on every frame.
  const lang = $derived(view.lang ?? 'text');
  const rawLines = $derived(text.split('\n'));
  const contextStart = $derived(view.context_start ?? 1);
  const start = $derived(view.start ?? contextStart);
  const end = $derived(view.end ?? contextStart + rawLines.length - 1);
  // 0-based slice bounds into the shipped context, clamped: the display copy may
  // be line-clipped below the claimed span for a huge read.
  const from = $derived(Math.max(start - contextStart, 0));
  const to = $derived(Math.max(from, Math.min(end - contextStart + 1, rawLines.length)));
  const shown = $derived(rawLines.slice(from, to));
  const gutterCh = $derived(String(end).length);

  const label = $derived(view.label ?? '');
  const slash = $derived(label.lastIndexOf('/'));
  const fileName = $derived(slash >= 0 ? label.slice(slash + 1) : label);
  const dirPath = $derived(slash >= 0 ? label.slice(0, slash + 1) : '');
  const icon = $derived(view.file === false ? valueIcon() : fileIcon(fileName));

  // Quiet span meta: the slice when partial, just the length when whole. A
  // display copy clipped below the read span says so instead of posing as it.
  const clipped = $derived(view.truncated === true || shown.length < end - start + 1);
  const meta = $derived.by(() => {
    const note = clipped ? ` · first ${shown.length} shown` : '';
    if (view.total != null && start === 1 && end === view.total)
      return `${view.total} lines${note}`;
    const of = view.total != null ? ` / ${view.total}` : '';
    return `${start}–${end}${of}${note}`;
  });

  // Per-line highlighted HTML for the whole context (null until the highlighter
  // loads; raw text shows meanwhile and upgrades in place).
  let lineHtml = $state<string[] | null>(null);
  $effect(() => {
    const src = text;
    const l = lang;
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

<div
  class="fv"
  style="--code-bg: light-dark({palette.light.bg}, {palette.dark.bg}); --code-lnr: light-dark({palette
    .light.line_nr}, {palette.dark.line_nr})"
>
  <header class="fv-head">
    <!-- Vendored static icon markup (see $lib/icons), safe to inject. -->
    <!-- eslint-disable-next-line svelte/no-at-html-tags -->
    <span class="fv-icon">{@html icon}</span>
    <span class="fv-name">{fileName || '(text)'}</span>
    {#if dirPath}<span class="fv-dir">{dirPath}</span>{/if}
    <span class="fv-meta">{meta}</span>
  </header>
  <div class="fv-code" style="--gutter: {gutterCh}ch">
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
    background: var(--code-bg);
  }
  .fv-head {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 7px 12px;
    border-bottom: 1px solid var(--edge);
    min-width: 0;
  }
  .fv-icon {
    flex: none;
    display: flex;
    align-items: center;
  }
  .fv-icon :global(svg) {
    width: 15px;
    height: 15px;
    display: block;
  }
  .fv-name {
    flex: none;
    font-size: 12px;
    color: var(--ink);
  }
  .fv-dir {
    font-size: 11.5px;
    color: var(--ink-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  .fv-meta {
    margin-left: auto;
    flex: none;
    font-size: 10.5px;
    color: var(--ink-faint);
    font-variant-numeric: tabular-nums;
  }
  .fv-code {
    overflow: auto;
    max-height: 60vh;
    padding: 10px 0;
  }
  .fv-row {
    display: flex;
    align-items: baseline;
    line-height: 1.55;
    font-size: 11.5px;
    white-space: pre;
  }
  .fv-ln {
    flex: none;
    width: calc(var(--gutter) + 3ch);
    padding-right: 1.5ch;
    text-align: right;
    color: var(--code-lnr);
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
