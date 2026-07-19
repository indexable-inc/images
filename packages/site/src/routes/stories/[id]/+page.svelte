<script lang="ts">
  import { resolve } from '$app/paths';
  import type { PageData } from './$types';

  const { data }: { data: PageData } = $props();
  const Body = $derived(data.story.component);
</script>

<svelte:head>
  <title>{data.story.title} · index</title>
  <meta name="description" content={data.story.hook} />
</svelte:head>

<article>
  <p class="eyebrow"><a href={resolve('/stories')}>Stories</a></p>
  <h1>{data.story.title}</h1>
  <p class="subtitle">{data.story.hook}</p>
  <div class="body">
    <Body />
  </div>
</article>

<style>
  .eyebrow {
    font-family: var(--font-mono);
    font-size: 0.75rem;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    margin: 0 0 -0.5rem;
  }

  .eyebrow a {
    color: var(--fg-faint);
  }
</style>
