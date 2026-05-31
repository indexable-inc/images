<script lang="ts">
  import { untrack } from 'svelte';
  import { renderJson } from '$lib/json';
  import type { Pane } from '$lib/types';

  // The data renderer: the pane's `body` is JSON. This is the generic tree (and
  // the `kv` renderer, and the fallback for any unknown kind). Built imperatively
  // so a deep structure does not allocate a Svelte component per node.
  let { pane }: { pane: Pane } = $props();
  let treeEl: HTMLElement | undefined = $state();
  const body = $derived(pane.body ?? '');

  $effect(() => {
    void body;
    const el = treeEl;
    if (!el) return;
    untrack(() => renderJson(el, body));
  });
</script>

<div class="json" bind:this={treeEl}></div>
