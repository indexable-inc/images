<script lang="ts">
  // The producer's rich HTML tables for an exec run: one self-contained document
  // per displayed DataFrame / eval result (see `_collect_html` in
  // python_worker.py). Each mounts in a sandboxed frame — `allow-scripts` without
  // `allow-same-origin` — so the in-frame click-to-sort script runs but the
  // document cannot reach the host page, its cookies, or its storage. The frame
  // scrolls internally with a sticky header so a tall table stays within the card;
  // `expanded` (the detail/feed view) gives it more room.
  let { docs, expanded = false }: { docs: string[]; expanded?: boolean } = $props();
</script>

{#each docs as doc, i (i)}
  <iframe
    class="exec-html"
    class:expanded
    title="table {i + 1}"
    sandbox="allow-scripts"
    srcdoc={doc}
  ></iframe>
{/each}

<style>
  .exec-html {
    display: block;
    width: 100%;
    height: 280px;
    border: 1px solid var(--line, #2a2a2a);
    background: transparent;
  }
  .exec-html.expanded {
    height: 60vh;
  }
  .exec-html + .exec-html {
    margin-top: 6px;
  }
</style>
