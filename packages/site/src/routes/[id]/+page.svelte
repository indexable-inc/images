<script lang="ts">
  import { onMount } from 'svelte';
  import UpdateEntry from '$lib/UpdateEntry.svelte';
  import { plainText } from '$lib/updates';
  import type { PageData } from './$types';

  const { data }: { data: PageData } = $props();

  let timeZone = $state<string | undefined>(undefined);
  onMount(() => {
    timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  });

  const titleText = $derived(plainText(data.update.title));
</script>

<svelte:head>
  <title>{titleText} · index</title>
  <meta name="description" content={titleText} />
</svelte:head>

<article class="entry">
  <UpdateEntry update={data.update} {timeZone} titleTag="h1" />
</article>
