<script lang="ts">
  import type { RichOutput } from '$lib/types';
  let { output }: { output: RichOutput } = $props();

  const data = $derived(output.data ?? {});
  // Pick the richest representation the bundle offers, in display priority.
  const png = $derived(data['image/png']);
  const jpeg = $derived(data['image/jpeg']);
  const svg = $derived(data['image/svg+xml']);
  const html = $derived(data['text/html']);
  const markdown = $derived(data['text/markdown']);
  const plain = $derived(data['text/plain']);
</script>

{#if png}
  <img class="img" src={`data:image/png;base64,${png}`} alt="" />
{:else if jpeg}
  <img class="img" src={`data:image/jpeg;base64,${jpeg}`} alt="" />
{:else if svg}
  <!-- agent-produced SVG; the dashboard trust boundary is the tailnet -->
  <div class="rich">{@html svg}</div>
{:else if html}
  <!-- agent-produced HTML (e.g. a DataFrame table); injected as-is -->
  <div class="rich">{@html html}</div>
{:else if markdown}
  <pre>{markdown}</pre>
{:else if plain}
  <pre class="res">{plain}</pre>
{/if}

<style>
  .img {
    display: block;
    max-width: 100%;
    margin: 8px 0 0;
    border: 1px solid var(--line);
    background: #fff;
  }
  pre {
    margin: 8px 0 0;
    max-height: 340px;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-word;
    font-size: 12px;
    color: var(--dim);
  }
  pre.res {
    color: var(--text);
  }
</style>
