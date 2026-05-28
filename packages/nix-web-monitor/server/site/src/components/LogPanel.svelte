<script lang="ts">
  import { tick } from 'svelte';
  import type { LogEntry } from '../types';

  type Props = {
    logs: ReadonlyArray<LogEntry>;
  };

  const RECENT_LOG_LIMIT = 500;

  const { logs }: Props = $props();

  type LevelFilter = 'all' | 'error' | 'warn' | 'info';
  let levelFilter = $state<LevelFilter>('all');
  let search = $state('');
  let stream = $state<HTMLDivElement | null>(null);
  /// When true, append-on-update keeps the view glued to the bottom. The
  /// scroll handler flips this off the moment the user scrolls up, and back
  /// on if they scroll back to the end.
  let follow = $state(true);

  const filtered = $derived(filterLogs(logs, levelFilter, search));
  const visible = $derived(filtered.slice(-RECENT_LOG_LIMIT));
  const hiddenCount = $derived(logs.length - visible.length);

  $effect(() => {
    // Re-run whenever visible changes; the dependency is read here.
    void visible.length;
    const target = stream;
    if (!follow || target === null) return;
    void tick().then(() => {
      target.scrollTop = target.scrollHeight;
    });
  });

  function filterLogs(
    items: ReadonlyArray<LogEntry>,
    level: LevelFilter,
    query: string
  ): ReadonlyArray<LogEntry> {
    const lower = query.trim().toLowerCase();
    return items.filter((entry) => {
      if (!matchesLevel(entry.level, level)) return false;
      if (lower.length === 0) return true;
      return entry.text.toLowerCase().includes(lower);
    });
  }

  function matchesLevel(level: number | null, filter: LevelFilter): boolean {
    if (filter === 'all') return true;
    if (filter === 'error') return level === 0;
    if (filter === 'warn') return level === 0 || level === 1;
    return level === null || level <= 3;
  }

  function lineClass(level: number | null): string {
    switch (level) {
      case 0:
        return 'log-error';
      case 1:
        return 'log-warn';
      case 2:
        return 'log-notice';
      case 3:
        return '';
      default:
        return level === null ? '' : 'log-debug';
    }
  }

  function onScroll(): void {
    if (stream === null) return;
    const distanceFromBottom = stream.scrollHeight - stream.scrollTop - stream.clientHeight;
    follow = distanceFromBottom <= 4;
  }

  function jumpToEnd(): void {
    follow = true;
    if (stream !== null) stream.scrollTop = stream.scrollHeight;
  }
</script>

<section class="panel logs-panel">
  <header class="panel-title">
    <span>logs</span>
    <div class="log-controls">
      <div class="filter-chips" role="tablist" aria-label="log level filter">
        {#each ['all', 'error', 'warn', 'info'] as const as choice (choice)}
          <button
            type="button"
            class="chip"
            class:active={levelFilter === choice}
            onclick={() => (levelFilter = choice)}
          >
            {choice}
          </button>
        {/each}
      </div>
      <input
        class="search"
        type="search"
        placeholder="filter"
        bind:value={search}
      />
      {#if !follow}
        <button type="button" class="chip jump" onclick={jumpToEnd}>jump &darr;</button>
      {/if}
      <span class="panel-meta">
        {String(visible.length)}{#if hiddenCount > 0} / {String(logs.length)}{/if}
      </span>
    </div>
  </header>
  <div class="log-stream" bind:this={stream} onscroll={onScroll}>
    {#each visible as log (log.index)}
      <div class="line {lineClass(log.level)}">
        <span class="idx">{String(log.index).padStart(5, '0')}</span>
        <span class="text">{log.text}</span>
      </div>
    {:else}
      <div class="empty">waiting for logs</div>
    {/each}
  </div>
</section>
