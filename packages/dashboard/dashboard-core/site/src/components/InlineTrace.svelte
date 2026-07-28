<script lang="ts">
  import { highlightLines } from '$lib/highlight';

  // Inline-trace execution: the code shown once, full width. Each line that
  // produced output shows its first output line inline in gray beside the code; a
  // line whose output has more lines is hoverable to reveal the full output in a
  // popover that *overlays* (absolute-positioned) rather than expanding the row,
  // so nothing shifts. This is the "inline evaluation" idea — Bret Victor's
  // "Inventing on Principle", Light Table's instarepl, Python Tutor, marimo —
  // applied to a captured run. The producer (python_worker.py) tags each stdout
  // write with its source line.
  let {
    source,
    lang = 'text',
    trace = [],
  }: { source: string; lang?: string; trace?: { line: number; text: string }[] } = $props();

  const lines = $derived(source.split('\n'));

  // Imports are boilerplate — folded away by default behind a one-line toggle.
  const isImport = (s: string): boolean => /^\s*(import\s|from\s+\S+\s+import\b)/.test(s);
  const importRows = $derived(new Set(lines.map((l, i) => (isImport(l) ? i : -1)).filter((i) => i >= 0)));
  let showImports = $state(false);

  // Output per 1-based source line, concatenated in emission order, trailing
  // newline trimmed (it would otherwise show as a blank line in the popover).
  const outByLine = $derived.by(() => {
    const map = new Map<number, string>();
    for (const t of trace) map.set(t.line, (map.get(t.line) ?? '') + t.text);
    for (const [k, v] of map) map.set(k, v.replace(/\n$/, ''));
    return map;
  });

  // The output's first line, shown inline (gray) next to the code as a preview;
  // the popover holds the full thing on hover. A trailing "+N" hints at more lines.
  function inlinePreview(out: string): string {
    const nl = out.indexOf('\n');
    if (nl === -1) return out;
    const more = out.slice(nl + 1).split('\n').length;
    return `${out.slice(0, nl)}  +${more}`;
  }
  function isMultiline(out: string): boolean {
    return out.includes('\n');
  }

  // Per-line highlighted HTML (null until the highlighter loads; raw text shows
  // meanwhile and upgrades in place).
  let lineHtml = $state<string[] | null>(null);
  $effect(() => {
    const src = source;
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

<div class="trace">
  {#if importRows.size > 0 && !showImports}
    <button class="trace-fold" onclick={() => (showImports = true)}>
      ⋯ {importRows.size} import{importRows.size === 1 ? '' : 's'}
    </button>
  {/if}
  {#each lines as line, i (i)}
    {#if showImports || !importRows.has(i)}
      {@const out = outByLine.get(i + 1)}
      <div class="trace-row" class:has-out={out !== undefined} class:multi={out !== undefined && isMultiline(out)}>
        <span class="trace-content">
          <span class="trace-code">
            {#if lineHtml && lineHtml[i] !== undefined}
              <!-- shiki escapes token text, so injecting one highlighted line is safe. -->
              <!-- eslint-disable-next-line svelte/no-at-html-tags -->
              {@html lineHtml[i]}
            {:else}<span class="trace-rawline">{line || ' '}</span>{/if}
          </span>
          {#if out !== undefined}<span class="trace-inline">{inlinePreview(out)}</span>{/if}
        </span>
        {#if out !== undefined && isMultiline(out)}
          <!-- Full output overlay, shown on hover; absolute so it never shifts code. -->
          <div class="trace-pop"><pre>{out}</pre></div>
        {/if}
      </div>
    {/if}
  {/each}
</div>
