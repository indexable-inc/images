<script lang="ts">
  import { onMount } from 'svelte';
  import { resolve } from '$app/paths';
  import FilterBar from '$lib/FilterBar.svelte';
  import UpdateEntry from '$lib/UpdateEntry.svelte';
  import { compileSearch, tagOptions } from '$lib/filter-expression';
  import { plainText, siteIntro, siteUpdates } from '$lib/updates';

  // The prerendered HTML uses UTC so every visitor's pre-hydration view
  // matches. After mount, we re-render each <time> in the visitor's local
  // zone. The `<time datetime>` attribute always carries the full ISO offset.
  let timeZone = $state<string | undefined>(undefined);
  onMount(() => {
    timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  });

  // Default search narrows to author-flagged headline items. Visitors can
  // clear the input to see the full log, add tags (autocompleted), or type
  // free text to full-text search titles and bodies.
  let filter = $state('interesting');

  const tags = tagOptions(siteUpdates);
  const tagNames = tags.map((t) => t.name);
  // Search corpus per entry: title + raw body, computed once.
  const searchable = siteUpdates.map((update) => ({
    update,
    candidate: { tags: update.tags, text: `${plainText(update.title)}\n${update.rawBody}` }
  }));

  const matches = $derived(compileSearch(filter, tagNames));
  const filtered = $derived(
    searchable.filter((s) => matches(s.candidate)).map((s) => s.update)
  );
</script>

<svelte:head>
  <title>index</title>
  <meta name="description" content={siteIntro} />
</svelte:head>

<section class="hero">
  <h1>Land it now. Everyone gets it.</h1>
  <p>{siteIntro}</p>
</section>

<FilterBar
  value={filter}
  onChange={(next: string) => {
    filter = next;
  }}
  matchCount={filtered.length}
  totalCount={siteUpdates.length}
  {tags}
/>

<ol class="log">
  {#each filtered as update (update.id)}
    <li>
      <UpdateEntry {update} {timeZone} permalinkBase={resolve('/')} />
    </li>
  {/each}
</ol>
