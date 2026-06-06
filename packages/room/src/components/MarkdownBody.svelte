<script lang="ts">
  import { renderMarkdown, highlighterReady } from '$lib/markdown';

  interface Props {
    source: string;
  }

  let { source }: Props = $props();

  // Track when shiki finishes loading so already-mounted messages
  // re-derive once and pick up syntax highlighting in place.
  let ready = $state(false);
  const unsub = highlighterReady.subscribe((v) => (ready = v));
  $effect(() => () => unsub());

  let html = $derived.by(() => {
    void ready; // re-derive when highlighter becomes available
    return renderMarkdown(source);
  });
</script>

<!-- eslint-disable-next-line svelte/no-at-html-tags -->
<div class="md">{@html html}</div>

<style>
  .md {
    color: var(--text);
    font-size: 12.5px;
    line-height: 1.55;
  }
</style>
