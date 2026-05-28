<script lang="ts">
  import { tick } from 'svelte';
  import { SvelteSet } from 'svelte/reactivity';
  import PanelHeader from '$lib/PanelHeader.svelte';
  import { LOG_LEVEL_FILTERS, type LogEntry, type LogLevelFilter } from '$lib/types';

  type Props = {
    logs: LogEntry[];
    selectedActivityId: number | null;
    onclearselection: () => void;
  };

  const RECENT_LOG_LIMIT = 500;

  const { logs, selectedActivityId, onclearselection }: Props = $props();

  let level = $state<LogLevelFilter>('all');
  let search = $state('');
  let stream = $state<HTMLDivElement | null>(null);
  let searchInput = $state<HTMLInputElement | null>(null);

  /// Imperative entry point for the errors panel: pin the log view to a single
  /// error line. Exposed through `bind:this` so the panel can drive the filter
  /// without the shell having to own log-view state.
  export function inspect(text: string): void {
    level = 'error';
    search = text;
  }
  /// When true, append-on-update keeps the view glued to the bottom. The
  /// scroll handler flips this off the moment the user scrolls up, and back
  /// on if they scroll back to the end.
  let follow = $state(true);
  /// Set of log indices the user has chosen to expand from the default
  /// single-line truncation.
  const expanded = new SvelteSet<number>();

  const filtered = $derived(filterLogs(logs, level, search, selectedActivityId));
  const visible = $derived(filtered.slice(-RECENT_LOG_LIMIT));
  const hiddenCount = $derived(logs.length - visible.length);

  $effect(() => {
    void visible.length;
    const target = stream;
    if (!follow || target === null) return;
    void tick().then(() => {
      target.scrollTop = target.scrollHeight;
    });
  });

  function filterLogs(
    items: LogEntry[],
    filter: LogLevelFilter,
    query: string,
    activityId: number | null
  ): LogEntry[] {
    const lower = query.trim().toLowerCase();
    return items.filter((entry) => {
      if (activityId !== null && entry.activityId !== activityId) return false;
      if (!matchesLevel(entry.level, filter)) return false;
      if (lower.length === 0) return true;
      return entry.text.toLowerCase().includes(lower);
    });
  }

  function matchesLevel(entryLevel: number | null, filter: LogLevelFilter): boolean {
    if (filter === 'all') return true;
    if (filter === 'error') return entryLevel === 0;
    if (filter === 'warn') return entryLevel === 0 || entryLevel === 1;
    return entryLevel === null || entryLevel <= 3;
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

  function isMultiline(text: string): boolean {
    return text.length > 120 || text.includes('\n');
  }

  function toggleExpanded(index: number): void {
    if (expanded.has(index)) expanded.delete(index);
    else expanded.add(index);
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

  /// Power-user shortcuts: `/` focuses the filter, `Esc` peels back the current
  /// filter then the build selection, `g`/`G` jumps to the live tail. Typing in
  /// a field is left alone except for `Esc`, which still clears the filter.
  function onWindowKeydown(event: KeyboardEvent): void {
    const target = event.target;
    const typing =
      target instanceof HTMLElement &&
      (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable);

    if (event.key === '/' && !typing) {
      event.preventDefault();
      searchInput?.focus();
      searchInput?.select();
      return;
    }
    if (event.key === 'Escape') {
      if (search.length > 0) {
        search = '';
      } else if (selectedActivityId !== null) {
        onclearselection();
      } else if (target === searchInput) {
        searchInput?.blur();
      }
      return;
    }
    if ((event.key === 'g' || event.key === 'G') && !typing) {
      event.preventDefault();
      jumpToEnd();
    }
  }
</script>

<svelte:window onkeydown={onWindowKeydown} />

<section class="panel logs-panel">
  <PanelHeader title="logs">
    <div class="log-controls">
      <div class="filter-chips" role="tablist" aria-label="log level filter">
        {#each LOG_LEVEL_FILTERS as choice (choice)}
          <button
            type="button"
            class="chip"
            class:active={level === choice}
            onclick={() => (level = choice)}
          >
            {choice}
          </button>
        {/each}
      </div>
      <input
        class="search"
        type="search"
        placeholder="filter  (/)"
        bind:this={searchInput}
        bind:value={search}
      />
      {#if selectedActivityId !== null}
        <button type="button" class="chip selection" onclick={onclearselection}>
          build #{String(selectedActivityId)} &times;
        </button>
      {/if}
      {#if !follow}
        <button type="button" class="chip jump" onclick={jumpToEnd}>jump &darr;</button>
      {/if}
      <span class="panel-meta">
        {String(visible.length)}{#if hiddenCount > 0} / {String(logs.length)}{/if}
      </span>
    </div>
  </PanelHeader>
  <div class="log-stream" bind:this={stream} onscroll={onScroll}>
    {#each visible as log (log.index)}
      {@const long = isMultiline(log.text)}
      {@const open = expanded.has(log.index)}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <div
        class="line {lineClass(log.level)}"
        class:expandable={long}
        class:expanded={open}
        role={long ? 'button' : undefined}
        tabindex={long ? 0 : undefined}
        onclick={long
          ? () => {
              toggleExpanded(log.index);
            }
          : undefined}
        onkeydown={long
          ? (event) => {
              if (event.key === 'Enter' || event.key === ' ') {
                event.preventDefault();
                toggleExpanded(log.index);
              }
            }
          : undefined}
        title={long && !open ? log.text : undefined}
      >
        <span class="idx">{String(log.index).padStart(5, '0')}</span>
        <span class="text">{log.text}</span>
      </div>
    {:else}
      <div class="empty">waiting for logs</div>
    {/each}
  </div>
</section>
