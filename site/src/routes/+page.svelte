<script lang="ts">
  import { onMount } from 'svelte';
  import FilterBar from '$lib/FilterBar.svelte';
  import UpdateEntry from '$lib/UpdateEntry.svelte';
  import { parseFilter } from '$lib/filter-expression';
  import { siteIntro, siteUpdates } from '$lib/updates';

  // The prerendered HTML uses UTC so every visitor's pre-hydration view
  // matches. After mount, we re-render each <time> in the visitor's local
  // zone. The `<time datetime>` attribute always carries the full ISO offset.
  let timeZone = $state<string | undefined>(undefined);
  onMount(() => {
    timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  });

  // Default filter narrows to author-flagged headline items. Visitors can
  // clear the input to see the full log or write any boolean expression.
  let filter = $state('interesting');

  const parsed = $derived(parseFilter(filter));
  const filtered = $derived(
    parsed.ok ? siteUpdates.filter((u) => parsed.matches(u.tags)) : siteUpdates
  );
  const error = $derived(parsed.ok ? undefined : parsed.error);
</script>

<svelte:head>
  <title>index</title>
  <meta name="description" content={siteIntro} />
</svelte:head>

<section class="hero">
  <h1>Open source from ix.</h1>
  <p>{siteIntro}</p>
</section>

<FilterBar
  value={filter}
  onChange={(next: string) => {
    filter = next;
  }}
  matchCount={filtered.length}
  totalCount={siteUpdates.length}
  {error}
/>

<ol class="log">
  {#each filtered as update (update.id)}
    <li>
      <UpdateEntry {update} {timeZone} />
    </li>
  {/each}
</ol>
