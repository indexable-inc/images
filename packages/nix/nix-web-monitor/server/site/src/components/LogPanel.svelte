<script lang="ts">
  import { tick } from 'svelte';
  import { splitDerivation } from '$lib/format';
  import { getPaneVisibility } from '$lib/panes/context';
  import { LOG_LEVEL_FILTERS, type LogEntry, type LogLevelFilter } from '$lib/types';

  type Props = {
    logs: LogEntry[];
    selectedActivityId: number | null;
    /// Derivation whose logs are pinned, so the filter chip names the build
    /// instead of an opaque activity id. Null when nothing is selected.
    selectedDrv: string | null;
    onclearselection: () => void;
  };

  const RECENT_LOG_LIMIT = 500;

  const { logs, selectedActivityId, selectedDrv, onclearselection }: Props = $props();

  /// Whether this pane is currently shown. Hidden panes stay mounted, so the
  /// window keydown handler below keeps firing and must stand down -- a hidden
  /// log pane must not swallow `/`, focus its invisible search input, or let
  /// Esc clear filters behind the operator's back.
  const paneVisible = getPaneVisibility();

  /// Package name of the pinned build, for the selection chip.
  const selectedName = $derived(selectedDrv === null ? null : splitDerivation(selectedDrv).name);

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

  function onScroll(): void {
    if (stream === null) return;
    const distanceFromBottom = stream.scrollHeight - stream.scrollTop - stream.clientHeight;
    follow = distanceFromBottom <= 4;
  }

  function jumpToEnd(): void {
    follow = true;
    if (stream !== null) stream.scrollTop = stream.scrollHeight;
  }

  /// Log shortcuts: `/` focuses the filter and `Esc` peels back the current
  /// filter then the build selection. The build tree owns `j/k/h/l` and `g`/`G`
  /// (jump to the live tail stays a button), so the two window handlers never
  /// contend for the same key. Typing in a field is left alone except for `Esc`.
  function onWindowKeydown(event: KeyboardEvent): void {
    if (!paneVisible()) return;
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
    }
  }
</script>

<svelte:window onkeydown={onWindowKeydown} />

<!-- Collapsing/expanding the drawer is the pane system's job now; this panel
     only owns the stream and its filters. -->
<section class="panel logs-panel">
  <div class="pane-toolbar">
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
        <button
          type="button"
          class="chip selection"
          title={selectedDrv ?? undefined}
          onclick={onclearselection}
        >
          {selectedName ?? `build #${String(selectedActivityId)}`} &times;
        </button>
      {/if}
      {#if !follow}
        <button type="button" class="chip jump" onclick={jumpToEnd}>jump &darr;</button>
      {/if}
      <span class="panel-meta">
        {String(visible.length)}{#if hiddenCount > 0} / {String(logs.length)}{/if}
      </span>
    </div>
  </div>
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
