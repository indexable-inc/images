<script lang="ts">
  import ContribNews from '$lib/components/ContribNews.svelte';
  import type { NewsItem } from '$lib/components/ContribNews.svelte';

  // Sample data: the last 24h of merges across ix + index.
  // TODO: Automate data collection from GitHub API and generate copy via Claude.
  const newsItems: NewsItem[] = [
    {
      headline: 'ix now has a federated Resources layer',
      description: 'Resources are addressable across machines and can be listed, read, and driven over QUIC. Enables cross-host resource discovery and manipulation.',
      prUrl: 'https://github.com/indexable-inc/ix/pull/5200',
      prNumber: '5200',
      category: 'Lead',
      timestamp: new Date(Date.now() - 2 * 60 * 60 * 1000) // 2h ago
    },
    {
      headline: 'The MCP kernel search runs on rg, fd and Spotlight',
      description: 'Search primitives now use ripgrep and fd internally with no fff in the loop. Faster, more reliable searching across codebases.',
      prUrl: 'https://github.com/indexable-inc/index/pull/1380',
      prNumber: '1380',
      category: 'Tooling',
      timestamp: new Date(Date.now() - 4 * 60 * 60 * 1000) // 4h ago
    },
    {
      headline: 'Antithesis is a continuous signal',
      description: 'Health and CI gates now read Antithesis results as pass or fail. Test runs integrate directly into the merge pipeline.',
      prUrl: 'https://github.com/indexable-inc/ix/pull/5195',
      prNumber: '5195',
      category: 'Antithesis',
      timestamp: new Date(Date.now() - 6 * 60 * 60 * 1000) // 6h ago
    },
    {
      headline: 'CAS keeps oversized manifests on disk',
      description: 'The content-addressable store now handles large manifests efficiently. VM runtime remains async end to end.',
      prUrl: 'https://github.com/indexable-inc/ix/pull/5190',
      prNumber: '5190',
      category: 'Storage',
      timestamp: new Date(Date.now() - 8 * 60 * 60 * 1000) // 8h ago
    },
    {
      headline: 'The build gate is a lint stage feeding rust and nix in parallel',
      description: 'CI now runs clippy and nixfmt checks in parallel before proceeding to builds. Faster feedback on style violations.',
      prUrl: 'https://github.com/indexable-inc/index/pull/1378',
      prNumber: '1378',
      category: 'CI',
      timestamp: new Date(Date.now() - 10 * 60 * 60 * 1000) // 10h ago
    },
    {
      headline: 'VCFS is free of the unlink race',
      description: 'Large-file unlink no longer fails with EFBIG. Virtual filesystem now handles cleanup safely under concurrent access.',
      prUrl: 'https://github.com/indexable-inc/ix/pull/5185',
      prNumber: '5185',
      category: 'Filesystem',
      timestamp: new Date(Date.now() - 12 * 60 * 60 * 1000) // 12h ago
    }
  ];
</script>

<svelte:head>
  <title>The Indexable Times | Last 24 Hours</title>
  <meta name="description" content="The last 24 hours of merges across ix and index, presented as news stories." />
</svelte:head>

<section class="newspaper">
  <header class="masthead">
    <h1>The Indexable Times</h1>
    <p class="subtitle">Last 24 hours of development across ix and index</p>
    <p class="note">
      <em>WIP: Data is currently hand-collected. Copy is generated via Claude. Automation coming soon.</em>
    </p>
  </header>

  <main class="news-feed">
    {#each newsItems as item, index (index)}
      <ContribNews {item} />
    {/each}
  </main>
</section>

<style>
  .newspaper {
    max-width: 800px;
    margin: 0 auto;
    padding: 2rem 1rem;
    font-family: Georgia, serif;
  }

  .masthead {
    text-align: center;
    margin-bottom: 2rem;
    border-bottom: 3px solid #333;
    padding-bottom: 1.5rem;
  }

  .masthead h1 {
    margin: 0;
    font-size: 3rem;
    font-weight: 700;
    letter-spacing: -1px;
  }

  .subtitle {
    margin: 0.5rem 0 0 0;
    font-size: 1.1rem;
    color: #666;
    font-style: italic;
  }

  .note {
    margin: 1rem 0 0 0;
    font-size: 0.9rem;
    color: #999;
  }

  .news-feed {
    margin-top: 2rem;
  }
</style>
