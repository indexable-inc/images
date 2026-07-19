<script lang="ts">
  import PlanEntry from '$lib/PlanEntry.svelte';
  import { plainText } from '$lib/updates';
  import type { PageData } from './$types';

  const { data }: { data: PageData } = $props();

  const titleText = $derived(`Plan ${data.plan.number}: ${plainText(data.plan.title)}`);
</script>

<svelte:head>
  <title>{titleText} · index</title>
  <meta name="description" content={data.plan.description ? plainText(data.plan.description) : titleText} />
</svelte:head>

<PlanEntry plan={data.plan} />
