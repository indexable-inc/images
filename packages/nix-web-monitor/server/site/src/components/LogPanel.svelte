<script lang="ts">
  import type { LogEntry } from '../types';

  type Props = {
    logs: ReadonlyArray<LogEntry>;
  };

  const RECENT_LOG_LIMIT = 300;

  const { logs }: Props = $props();
  const recent = $derived(logs.slice(-RECENT_LOG_LIMIT));

  /// Map Nix's log level taxonomy onto presentation classes.
  /// Level taxonomy mirrors `Verbosity` in nix/src/libutil/logging.hh.
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
</script>

<section class="panel logs-panel">
  <div class="panel-title">logs</div>
  <pre>{#each recent as log (log.index)}<span class="line {lineClass(log.level)}"><span class="idx">{String(log.index).padStart(5, '0')}</span> {log.text}</span>
{:else}<span class="empty">waiting for logs</span>{/each}</pre>
</section>
