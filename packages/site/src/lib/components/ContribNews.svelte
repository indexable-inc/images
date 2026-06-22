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

  const categoryColors: Record<NewsItem['category'], { bg: string; text: string }> = {
    'Lead': { bg: '#dbeafe', text: '#111827' },
    'Tooling': { bg: '#f3e8ff', text: '#581c87' },
    'Antithesis': { bg: '#fee2e2', text: '#7f1d1d' },
    'Storage': { bg: '#fed7aa', text: '#92400e' },
    'CI': { bg: '#dcfce7', text: '#14532d' },
    'Filesystem': { bg: '#f3f4f6', text: '#1f2937' }
  };

  const formattedTime = item.timestamp.toLocaleTimeString('en-US', {
    hour: '2-digit',
    minute: '2-digit',
    hour12: true
  });
</script>

<article class="news-item">
  <div class="header">
    <span class="category" style="background-color: {categoryColors[item.category].bg}; color: {categoryColors[item.category].text}">
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
  .news-item {
    padding: 1rem;
    border-left: 4px solid #ccc;
    margin-bottom: 1.5rem;
    background: #fafafa;
  }

  .header {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    margin-bottom: 0.5rem;
  }

  .category {
    padding: 0.25rem 0.5rem;
    border-radius: 4px;
    font-size: 0.75rem;
    font-weight: 600;
    text-transform: uppercase;
  }

  .timestamp {
    font-size: 0.75rem;
    color: #666;
  }

  .headline {
    margin: 0.5rem 0;
    font-size: 1.1rem;
    font-weight: 600;
  }

  .description {
    margin: 0.5rem 0;
    color: #444;
    font-size: 0.9rem;
    line-height: 1.5;
  }

  .pr-link {
    display: inline-block;
    margin-top: 0.5rem;
    color: #0066cc;
    text-decoration: none;
    font-weight: 500;
  }

  .pr-link:hover {
    text-decoration: underline;
  }
</style>
