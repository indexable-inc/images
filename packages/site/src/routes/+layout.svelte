<script lang="ts">
  import type { Snippet } from 'svelte';
  import { resolve } from '$app/paths';
  import { siteFeedUrl } from '$lib/updates';
  import '../app.css';

  const { children }: { children: Snippet } = $props();

  const homeHref = resolve('/');
  const philosophyHref = resolve('/philosophy');
  const storiesHref = resolve('/stories');
  const rfcsHref = resolve('/rfcs');
  const feedHref = resolve('/feed.xml');
</script>

<svelte:head>
  <link rel="alternate" type="application/rss+xml" title="index" href={siteFeedUrl} />
</svelte:head>

<header class="site-header">
  <div class="bar">
    <a class="brand" href={homeHref} aria-label="index home">
      <span class="brand-mark">index</span>
    </a>
    <nav aria-label="Primary">
      <a href={philosophyHref}>philosophy</a>
      <a href={storiesHref}>stories</a>
      <a href={rfcsHref}>rfcs</a>
      <a href="https://github.com/indexable-inc/index">github</a>
      <a href="https://ix.dev">ix.dev</a>
      <a href={feedHref}>rss</a>
    </nav>
  </div>
</header>

<main>
  {@render children()}
</main>
