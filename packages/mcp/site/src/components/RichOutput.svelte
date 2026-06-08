<script lang="ts">
  import { type RichOutput, type LlmView, IX_LLM_MIME } from '$lib/types';
  import { view } from '$lib/view.svelte';
  let { output }: { output: RichOutput } = $props();

  const data = $derived(output.data ?? {});
  // Pick the richest representation the bundle offers, in display priority.
  const png = $derived(data['image/png']);
  const jpeg = $derived(data['image/jpeg']);
  const svg = $derived(data['image/svg+xml']);
  const html = $derived(data['text/html']);
  const markdown = $derived(data['text/markdown']);
  const plain = $derived(data['text/plain']);

  // The raw model-facing view, when the header toggle is on: the exact text and
  // images the agent received. IX_LLM_MIME carries both (text plus downscaled
  // images) when a Result had images; otherwise text/plain is the model's text.
  const llm = $derived.by((): LlmView => {
    const encoded = data[IX_LLM_MIME];
    if (encoded) {
      try {
        return JSON.parse(encoded) as LlmView;
      } catch {
        // Fall through to the text-only view below.
      }
    }
    return { text: plain ?? '', images: [] };
  });
</script>

{#if view.rawLLM}
  <!-- What the LLM actually saw: its concise text and any images it was sent. -->
  {#if llm.text}<pre class="res">{llm.text}</pre>{/if}
  {#each llm.images as img, i (i)}
    <img class="img" src={`data:${img.mime};base64,${img.data}`} alt="" />
  {/each}
  {#if !llm.text && llm.images.length === 0}
    <pre class="res dim">(no model output)</pre>
  {/if}
{:else if png}
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
  pre.dim {
    color: var(--faint);
    font-style: italic;
  }
</style>
