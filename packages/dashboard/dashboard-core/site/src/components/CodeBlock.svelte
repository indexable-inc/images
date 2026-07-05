<script lang="ts">
  import { highlightLines } from '$lib/highlight';

  // A code block, syntax-highlighted by shiki when possible. While the highlighter
  // loads (or for an unknown language / offline CDN) it shows the raw text, so the
  // code is always legible immediately and highlighting upgrades it in place.
  let {
    code,
    lang = 'text',
    line = null,
    errorLine = null,
  }: { code: string; lang?: string; line?: number | null; errorLine?: number | null } = $props();

  let html = $state<string | null>(null);
  const rawLines = $derived(code.split('\n'));

  function markLines(lines: string[], live: number | null, error: number | null): string {
    return lines
      .map((part, index) => {
        const n = index + 1;
        const cls = n === error ? 'cb-line cb-error' : n === live ? 'cb-line cb-live' : 'cb-line';
        return `<span class="${cls}" data-line="${n}">${part || ' '}</span>`;
      })
      .join('\n');
  }

  $effect(() => {
    // Track the inputs; a value-equal source (the live tail re-emits the same
    // string each frame) is `===` and does not re-run this.
    const c = code;
    const l = lang;
    const live = line;
    const err = errorLine;
    let alive = true;
    html = null;
    void highlightLines(c, l).then((out) => {
      if (alive) html = out ? markLines(out, live, err) : null;
    });
    return () => {
      alive = false;
    };
  });
</script>

{#if html}
  <!-- shiki escapes token text, so injecting its HTML is safe. -->
  <!-- eslint-disable-next-line svelte/no-at-html-tags -->
  {@html html}
{:else}
  <pre class="cb-raw">{#each rawLines as rawLine, index}<span class="cb-line" class:cb-live={line === index + 1} class:cb-error={errorLine === index + 1} data-line={index + 1}>{rawLine || ' '}</span>{/each}</pre>
{/if}

<style>
  :global(.cb-line) {
    display: block;
    padding: 0 0.6rem;
    margin: 0 -0.6rem;
    border-left: 2px solid transparent;
  }

  :global(.cb-live) {
    background: color-mix(in srgb, #7bd88f 14%, transparent);
    border-left-color: #7bd88f;
  }

  :global(.cb-error) {
    background: color-mix(in srgb, #fc618d 16%, transparent);
    border-left-color: #fc618d;
  }
</style>
