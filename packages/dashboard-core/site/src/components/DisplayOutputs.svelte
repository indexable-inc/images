<script lang="ts">
  import type { DisplayBundle } from '$lib/exec';

  // An exec run's rich-display outputs (see `_collect_displays` in
  // python_worker.py): one MIME bundle per displayed object / eval result /
  // figure. For each bundle we render the richest representation we know — a
  // `text/html` table/document, then an `image/*`, then text — mirroring the
  // Jupyter frontend's "pick the best mimetype" rule. Adding a new rich type is a
  // new branch here, with no change to the worker, the wire, or the hub.
  //
  // `text/html` mounts in a sandboxed frame (`allow-scripts`, no
  // `allow-same-origin`) so a producer's in-frame script (the table sort) runs but
  // the document cannot reach the host page, its cookies, or its storage. The
  // frame scrolls internally with a sticky header so a tall table stays within the
  // card; `expanded` (the detail/feed view) gives it more room.
  let { outputs, expanded = false }: { outputs: DisplayBundle[]; expanded?: boolean } = $props();

  // The richest MIME we have a renderer for, in preference order, or null.
  function pick(b: DisplayBundle): string | null {
    for (const mime of ['text/html', 'image/png', 'image/jpeg', 'text/latex', 'text/plain']) {
      if (b[mime]) return mime;
    }
    return null;
  }
</script>

{#each outputs as bundle, i (i)}
  {@const mime = pick(bundle)}
  {#if mime === 'text/html'}
    <iframe
      class="display-html"
      class:expanded
      title="output {i + 1}"
      sandbox="allow-scripts"
      srcdoc={bundle[mime]}
    ></iframe>
  {:else if mime === 'image/png' || mime === 'image/jpeg'}
    <img class="display-img" alt="output {i + 1}" src="data:{mime};base64,{bundle[mime]}" />
  {:else if mime}
    <pre class="display-text">{bundle[mime]}</pre>
  {/if}
{/each}

<style>
  .display-html {
    display: block;
    width: 100%;
    height: 280px;
    border: 1px solid var(--line, #2a2a2a);
    background: transparent;
  }
  .display-html.expanded {
    height: 60vh;
  }
  .display-img {
    display: block;
    max-width: 100%;
    height: auto;
  }
  .display-text {
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .display-html + .display-html,
  .display-img + .display-img,
  :global(.display-html) + .display-img,
  :global(.display-img) + .display-html {
    margin-top: 6px;
  }
</style>
