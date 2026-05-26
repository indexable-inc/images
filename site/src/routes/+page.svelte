<script lang="ts">
  import { resolve } from '$app/paths';
  import { Marked } from 'marked';
  import { siteFeedUrl, siteIntro, siteUpdates } from '$lib/updates';

  const safeHrefPattern = /^(https?:|mailto:|#|\/)/i;

  const marked = new Marked({
    gfm: true,
    breaks: false,
    renderer: {
      html: () => '',
      link({ href, title, tokens }) {
        const text = this.parser.parseInline(tokens);
        if (!safeHrefPattern.test(href)) return text;
        const titleAttr = title ? ` title="${title.replace(/"/g, '&quot;')}"` : '';
        return `<a href="${href}"${titleAttr}>${text}</a>`;
      }
    }
  });

  const feedHref = resolve('/feed.xml');

  const dateFormatter = new Intl.DateTimeFormat('en', {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
    timeZone: 'UTC'
  });

  function formatDate(date: string): string {
    return dateFormatter.format(new Date(`${date}T00:00:00Z`));
  }

  const entries = siteUpdates.map((update) => ({
    ...update,
    html: marked.parse(update.body) as string,
    label: formatDate(update.date)
  }));
</script>

<svelte:head>
  <title>ix images</title>
  <meta name="description" content={siteIntro} />
  <link rel="alternate" type="application/rss+xml" title="ix images" href={siteFeedUrl} />
</svelte:head>

<header>
  <a class="wordmark" href={resolve('/')}>ix images</a>
  <nav>
    <a href="https://github.com/indexable-inc/index">github</a>
    <a href="https://ix.dev">ix.dev</a>
    <a href={feedHref}>rss</a>
  </nav>
</header>

<main>
  <section class="hero">
    <h1>Pre-built systems for ix VMs.</h1>
    <p>{siteIntro}</p>
  </section>

  <ol class="log">
    {#each entries as entry (entry.id)}
      <li id={entry.id}>
        <time datetime={entry.date}>{entry.label}</time>
        <h2><a href="#{entry.id}">{entry.title}</a></h2>
        <!-- eslint-disable-next-line svelte/no-at-html-tags -->
        <div class="body">{@html entry.html}</div>
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
