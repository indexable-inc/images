<script lang="ts">
  import type { Snippet } from 'svelte';
  import { resolve } from '$app/paths';
  import { siteFeedUrl } from '$lib/updates';
  import '../app.css';

  const { children }: { children: Snippet } = $props();

  const homeHref = resolve('/');
  const rfcsHref = resolve('/rfcs');
  const feedHref = resolve('/feed.xml');
</script>

<svelte:head>
  <link rel="alternate" type="application/rss+xml" title="index" href={siteFeedUrl} />
</svelte:head>

<header>
  <a class="wordmark" href={homeHref}>index</a>
  <nav>
    <a href={rfcsHref}>rfcs</a>
    <a href="https://github.com/indexable-inc/index">github</a>
    <a href="https://ix.dev">ix.dev</a>
    <a href={feedHref}>rss</a>
  </nav>
</header>

<main>
  {@render children()}
</main>
