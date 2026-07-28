<script module lang="ts">
  export interface NewsItem {
    headline: string;
    description: string;
    prUrl: string;
    prNumber: string;
    category: 'Lead' | 'Tooling' | 'Antithesis' | 'Storage' | 'CI' | 'Filesystem';
    timestamp: Date;
  }
</script>

<script lang="ts">
  export let item: NewsItem;

  // Categories borrow the site's one semantic-color family (--status-*),
  // the same way ix reserves color for code syntax.
  const categoryColor: Record<NewsItem['category'], string> = {
    Lead: 'var(--status-accepted)',
    Tooling: 'var(--status-last-call)',
    Antithesis: 'var(--status-rejected)',
    Storage: 'var(--status-input-wanted)',
    CI: 'var(--status-load-bearing)',
    Filesystem: 'var(--status-sketch)'
  };

  const formattedTime = item.timestamp.toLocaleTimeString('en-US', {
    hour: '2-digit',
    minute: '2-digit',
    hour12: true
  });
</script>

<article class="news-item">
  <div class="header">
    <span class="category" style:--c={categoryColor[item.category]}>
      {item.category}
    </span>
    <time class="timestamp" dateTime={item.timestamp.toISOString()}>
      {formattedTime}
    </time>
  </div>
  <h3 class="headline">{item.headline}</h3>
  <p class="description">{item.description}</p>
  <!-- eslint-disable-next-line svelte/no-navigation-without-resolve -->
  <a href={item.prUrl} target="_blank" rel="noopener noreferrer" class="pr-link">
    #{item.prNumber}
  </a>
</article>

<style>
  /* One grid-aligned panel per merge; the 1px frame is absorbed into the
     vertical padding so following rows stay on the cell grid. */
  .news-item {
    border: 1px solid var(--rule);
    border-radius: var(--radius);
    padding: calc(var(--cell-h) - 1px) 2ch;
    margin-bottom: var(--cell-h);
    background: var(--bg);
  }

  .header {
    display: flex;
    gap: 0 2ch;
    align-items: baseline;
  }

  /* [ CATEGORY ] chip in the category's semantic color. */
  .category {
    font-weight: 700;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--c);
    white-space: nowrap;
  }

  .category::before {
    content: '[';
    font-weight: 400;
    color: color-mix(in srgb, var(--c) 55%, transparent);
  }

  .category::after {
    content: ']';
    font-weight: 400;
    color: color-mix(in srgb, var(--c) 55%, transparent);
  }

  .timestamp {
    display: inline;
    color: var(--fg-faint);
  }

  .headline {
    margin: 0;
  }

  .description {
    margin: 0;
    color: var(--fg-muted);
  }

  .pr-link {
    display: inline-block;
    color: var(--fg-muted);
    text-decoration: none;
  }

  .pr-link:hover {
    background: var(--fg);
    color: var(--bg);
  }
</style>
