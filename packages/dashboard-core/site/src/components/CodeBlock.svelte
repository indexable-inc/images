<script lang="ts">
  import { highlight } from '$lib/highlight';

  // A code block, syntax-highlighted by shiki when possible. While the highlighter
  // loads (or for an unknown language / offline CDN) it shows the raw text, so the
  // code is always legible immediately and highlighting upgrades it in place.
  let { code, lang = 'text' }: { code: string; lang?: string } = $props();

  let html = $state<string | null>(null);

  $effect(() => {
    // Track the inputs; a value-equal source (the live tail re-emits the same
    // string each frame) is `===` and does not re-run this.
    const c = code;
    const l = lang;
    let alive = true;
    html = null;
    void highlight(c, l).then((out) => {
      if (alive) html = out;
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
  <pre class="cb-raw">{code}</pre>
{/if}
