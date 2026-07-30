<script lang="ts">
  import { Skeleton } from '$lib/components/ui/skeleton';
  import type { AppState } from '$lib/store.svelte';

  // Takes the state as a prop rather than importing the store, so the same strip
  // renders a reconstructed past state when the reader scrubs the history.
  let { state }: { state: AppState } = $props();

  const loading = $derived(state.sections.filter((section) => section.loading));
</script>

<div
  class="flex items-center gap-2.5 rounded-lg border bg-card px-3.5 py-2.5 font-mono text-xs"
  role="status"
>
  {#if state.done}
    <span class="size-[7px] flex-none rounded-full border border-chart-2"></span>
  {:else}
    <span
      class="size-[7px] flex-none animate-pulse rounded-full bg-primary motion-reduce:animate-none"
    ></span>
  {/if}
  <span class="text-foreground">{state.status}</span>
  {#if loading.length > 0}
    <span class="ml-auto text-muted-foreground">{loading.length} loading</span>
  {/if}
</div>

{#if loading.length > 0}
  <div class="mt-2.5 flex flex-col gap-2.5">
    {#each loading as section (section.id)}
      <div class="flex flex-col gap-1.5 rounded-lg border border-dashed bg-card px-3.5 py-3">
        <span class="text-xs text-muted-foreground">{section.title}</span>
        <Skeleton class="h-2.5 w-[62%]" />
        <Skeleton class="h-2.5 w-[84%]" />
      </div>
    {/each}
  </div>
{/if}
