<script lang="ts">
  import { onMount } from 'svelte';
  import { resolve } from '$app/paths';
  import {
    inlineTitleHtml,
    siteFeedUrl,
    siteIntro,
    siteUpdates
  } from '$lib/updates';

  const feedHref = resolve('/feed.xml');

  // The prerendered HTML renders in UTC so it reads the same in every
  // visitor's zone before JS runs. After hydration the page reformats each
  // <time> in the visitor's local zone so the label matches their wall clock.
  // The <time datetime> attribute always carries the full ISO offset.
  let timeZone = $state<string | undefined>(undefined);

  onMount(() => {
    timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  });

  function formatPostedAt(postedAt: string, zone: string | undefined): string {
    const parsed = new Date(postedAt);
    const tz = zone ?? 'UTC';
    const date = new Intl.DateTimeFormat('en', {
      month: 'short',
      day: 'numeric',
      year: 'numeric',
      timeZone: tz
    }).format(parsed);
    const time = new Intl.DateTimeFormat('en', {
      hour: '2-digit',
      minute: '2-digit',
      hour12: false,
      timeZone: tz
    }).format(parsed);
    const tzNamePart = new Intl.DateTimeFormat('en', {
      timeZoneName: 'short',
      timeZone: tz
    })
      .formatToParts(parsed)
      .find((part) => part.type === 'timeZoneName');
    return `${date} · ${time} ${tzNamePart?.value ?? tz}`;
  }

  const entries = siteUpdates.map((update) => ({
    ...update,
    titleHtml: inlineTitleHtml(update.title)
  }));
</script>

<svelte:head>
  <title>index</title>
  <meta name="description" content={siteIntro} />
  <link rel="alternate" type="application/rss+xml" title="index" href={siteFeedUrl} />
</svelte:head>

<header>
  <a class="wordmark" href={resolve('/')}>index</a>
  <nav>
    <a href="https://github.com/indexable-inc/index">github</a>
    <a href="https://ix.dev">ix.dev</a>
    <a href={feedHref}>rss</a>
  </nav>
</header>

<main>
  <section class="hero">
    <h1>Open source from ix.</h1>
    <p>{siteIntro}</p>
  </section>

  <ol class="log">
    {#each entries as entry (entry.id)}
      {@const Entry = entry.component}
      <li id={entry.id}>
        <time datetime={entry.postedAt}>
          {formatPostedAt(entry.postedAt, timeZone)}
        </time>
        <h2>
          <!-- eslint-disable-next-line svelte/no-at-html-tags -->
          <a href="#{entry.id}">{@html entry.titleHtml}</a>
        </h2>
        <div class="body">
          <Entry />
        </div>
        <div class="refs">
          {#each entry.links as link, i (link.href)}
            {#if i > 0}<span aria-hidden="true">·</span>{/if}
            <a href={link.href} rel="external">{link.label}</a>
          {/each}
        </div>
      </li>
    {/each}
  </ol>
</main>
