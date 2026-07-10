<script lang="ts">
  import RfcEntry from '$lib/RfcEntry.svelte';
  import { plainText } from '$lib/updates';
  import type { PageData } from './$types';

  const { data }: { data: PageData } = $props();

  const titleText = $derived(`RFC ${data.rfc.number}: ${plainText(data.rfc.title)}`);
</script>

<svelte:head>
  <title>{titleText} · index</title>
  <meta name="description" content={data.rfc.description ? plainText(data.rfc.description) : titleText} />
</svelte:head>

<RfcEntry rfc={data.rfc} />
