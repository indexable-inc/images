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
    letter-spacing: 0.08em;
    text-transform: uppercase;
    margin: 0;
  }

  .eyebrow a {
    color: var(--fg-faint);
    text-decoration: none;
  }

  .eyebrow a:hover {
    background: var(--fg);
    color: var(--bg);
  }
</style>
