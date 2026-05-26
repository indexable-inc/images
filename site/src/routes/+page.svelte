<script lang="ts">
  import { onMount } from 'svelte';
  import UpdateEntry from '$lib/UpdateEntry.svelte';
  import { siteIntro, siteUpdates } from '$lib/updates';

  // The prerendered HTML uses UTC so every visitor's pre-hydration view
  // matches. After mount, we re-render each <time> in the visitor's local
  // zone. The `<time datetime>` attribute always carries the full ISO offset.
  let timeZone = $state<string | undefined>(undefined);
  onMount(() => {
    timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  });
</script>

<svelte:head>
  <title>index</title>
  <meta name="description" content={siteIntro} />
</svelte:head>

<section class="hero">
  <h1>Open source from ix.</h1>
  <p>{siteIntro}</p>
</section>

<ol class="log">
  {#each siteUpdates as update (update.id)}
    <li>
      <UpdateEntry {update} {timeZone} />
    </li>
  {/each}
</ol>
