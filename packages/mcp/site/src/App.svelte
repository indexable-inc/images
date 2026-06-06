<script lang="ts">
  import { feed } from '$lib/feed.svelte';
  import { now } from '$lib/now.svelte';
  import JobCard from '$components/JobCard.svelte';
  import ResourceCard from '$components/ResourceCard.svelte';

  // Oldest at top, newest at bottom: the feed reads like a notebook.
  const ordered = $derived([...feed.jobs].sort((a, b) => a.started_at - b.started_at));
  const running = $derived(feed.jobs.filter((j) => j.status === 'running').length);

  // Stick-to-bottom that never fights the user: we only re-pin to the bottom on
  // a refresh if the user was already near it. Scrolling up frees the view, and
  // because the list is keyed, scroll position is otherwise untouched.
  let nearBottom = true;
  function trackScroll(): void {
    nearBottom = window.innerHeight + window.scrollY >= document.body.scrollHeight - 120;
  }

  $effect(() => {
    feed.start();
    now.start();
    window.addEventListener('scroll', trackScroll, { passive: true });
    trackScroll();
    return () => {
      feed.stop();
      now.stop();
      window.removeEventListener('scroll', trackScroll);
    };
  });

  $effect(() => {
    // Re-run on every refresh (feed.jobs is reassigned each poll).
    void feed.jobs;
    if (nearBottom) {
      requestAnimationFrame(() => window.scrollTo(0, document.body.scrollHeight));
    }
  });
</script>

<header class="top">
  <span class="brand"><b>ix</b> &middot; mcp</span>
  <span class="spacer"></span>
  <span class="stat" class:stale={!feed.connected}>
    {#if running}<span class="dot"></span><b>{running}</b> running &nbsp;{/if}
    <b>{feed.jobs.length}</b> total
  </span>
</header>

<div class="wrap">
  <main>
    <div class="sec">executions</div>
    {#if ordered.length === 0}
      <div class="empty">no executions yet</div>
    {:else}
      {#each ordered as job (job.id)}
        <JobCard {job} />
      {/each}
    {/if}
  </main>

  <aside class="sidebar">
    <div class="sec">resources</div>
    {#if feed.resources.length === 0}
      <div class="empty">no live resources</div>
    {:else}
      {#each feed.resources as resource (resource.id)}
        <ResourceCard {resource} />
      {/each}
    {/if}
  </aside>
</div>

<style>
  .top {
    position: sticky;
    top: 0;
    z-index: 5;
    display: flex;
    gap: 12px;
    align-items: center;
    padding: 11px 18px;
    background: rgba(11, 11, 12, 0.86);
    backdrop-filter: blur(8px);
    border-bottom: 1px solid var(--line);
  }
  .brand {
    color: var(--dim);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.22em;
    text-transform: uppercase;
  }
  .brand :global(b) {
    color: var(--text);
    font-weight: 600;
  }
  .spacer {
    flex: 1;
  }
  .stat {
    color: var(--muted);
    font-size: 11px;
    letter-spacing: 0.04em;
  }
  .stat :global(b) {
    color: var(--text);
    font-weight: 600;
  }
  .stat.stale {
    color: var(--err);
  }
  .dot {
    display: inline-block;
    width: 6px;
    height: 6px;
    margin-right: 6px;
    background: var(--active);
    vertical-align: middle;
  }
  .wrap {
    display: flex;
    gap: 18px;
    align-items: flex-start;
    max-width: 1600px;
    margin: 0 auto;
    padding: 18px;
  }
  main {
    flex: 1 1 auto;
    min-width: 0;
  }
  .sidebar {
    position: sticky;
    top: 62px;
    flex: 0 0 520px;
    max-height: calc(100vh - 78px);
    overflow: auto;
  }
  @media (max-width: 1100px) {
    .wrap {
      flex-direction: column;
    }
    .sidebar {
      position: static;
      flex: none;
      width: 100%;
      max-height: none;
    }
  }
  .sec {
    margin: 0 0 12px;
    padding-bottom: 7px;
    color: var(--muted);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.2em;
    text-transform: uppercase;
    border-bottom: 1px solid var(--line);
  }
  .empty {
    padding: 2px 0;
    color: var(--faint);
    font-size: 12px;
    font-style: italic;
  }
</style>
