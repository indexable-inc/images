<script lang="ts">
  import { resolve } from '$app/paths';
  import { siteFeedUrl, siteIntro, siteUpdates } from '$lib/updates';

  const dateFormatter = new Intl.DateTimeFormat('en', {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
    timeZone: 'UTC'
  });

  const feedHref = resolve('/feed.xml');
  const latestUpdate = siteUpdates[0];

  let selectedId = $state(latestUpdate.id);

  const selectedUpdate = $derived(
    siteUpdates.find((update) => update.id === selectedId) ?? latestUpdate
  );

  function formatDate(date: string): string {
    return dateFormatter.format(new Date(`${date}T00:00:00Z`));
  }
</script>

<svelte:head>
  <title>ix images</title>
  <meta
    name="description"
    content="Pre-built OCI images and composable NixOS modules for ix VMs, with a compact RSS update feed."
  />
  <link rel="alternate" type="application/rss+xml" title="ix images updates" href={siteFeedUrl} />
</svelte:head>

<main>
  <header class="masthead" aria-labelledby="page-title">
    <div>
      <p class="eyebrow">ix images</p>
      <h1 id="page-title">Pre-built systems for ix VMs.</h1>
    </div>
    <nav class="top-links" aria-label="Primary links">
      <a href="https://github.com/indexable-inc/index">GitHub</a>
      <a href="https://ix.dev">ix.dev</a>
      <a href={feedHref}>RSS</a>
    </nav>
  </header>

  <section class="intro" aria-label="Project summary">
    <p>{siteIntro}</p>
    <p>
      This page is the public changelog: short entries, exact source links, and a feed
      that works in any RSS reader.
    </p>
  </section>

  <section class="updates" aria-labelledby="updates-title">
    <div class="section-heading">
      <p class="eyebrow">Updates</p>
      <h2 id="updates-title">Latest changes</h2>
    </div>

    <div class="update-layout">
      <ol class="update-list" aria-label="Update list">
        {#each siteUpdates as update (update.id)}
          <li>
            <button
              type="button"
              class:selected={update.id === selectedId}
              aria-pressed={update.id === selectedId}
              onclick={() => {
                selectedId = update.id;
              }}
            >
              <span class="update-meta">
                <time datetime={update.date}>{formatDate(update.date)}</time>
              </span>
              <span class="update-title">{update.title}</span>
              <span class="update-summary">{update.summary}</span>
            </button>
          </li>
        {/each}
      </ol>

      <article id={selectedUpdate.id} class="update-detail" aria-labelledby="selected-update-title">
        <time datetime={selectedUpdate.date}>{formatDate(selectedUpdate.date)}</time>
        <h3 id="selected-update-title">{selectedUpdate.title}</h3>
        <p class="summary">{selectedUpdate.summary}</p>
        {#each selectedUpdate.paragraphs as paragraph (paragraph)}
          <p>{paragraph}</p>
        {/each}
        <div class="link-row" aria-label="Update links">
          {#each selectedUpdate.links as link (link.href)}
            <a href={link.href} rel="external">{link.label}</a>
          {/each}
        </div>
      </article>
    </div>
  </section>

  <section class="repository" aria-labelledby="repository-title">
    <h2 id="repository-title">Source</h2>
    <p>
      Images are discovered from <code>images/</code>. NixOS modules live under
      <code>modules/</code>. The repository is
      <a href="https://github.com/indexable-inc/index">indexable-inc/index</a>.
    </p>
  </section>
</main>
