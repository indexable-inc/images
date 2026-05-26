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

  // Render in UTC so the prerendered HTML reads the same in every visitor's
  // zone. The <time datetime> attribute carries the full offset for clients
  // that want to reformat locally.
  const dateFormatter = new Intl.DateTimeFormat('en', {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
    timeZone: 'UTC'
  });
  const timeFormatter = new Intl.DateTimeFormat('en', {
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
    timeZone: 'UTC'
  });

  function formatPostedAt(postedAt: string): string {
    const parsed = new Date(postedAt);
    return `${dateFormatter.format(parsed)} · ${timeFormatter.format(parsed)} UTC`;
  }

  const entries = siteUpdates.map((update) => ({
    ...update,
    html: marked.parse(update.body) as string,
    titleHtml: marked.parseInline(update.title) as string,
    label: formatPostedAt(update.postedAt)
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
      <li id={entry.id}>
        <time datetime={entry.postedAt}>{entry.label}</time>
        <h2>
          <!-- eslint-disable-next-line svelte/no-at-html-tags -->
          <a href="#{entry.id}">{@html entry.titleHtml}</a>
        </h2>
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
