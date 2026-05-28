<script lang="ts">
  import type { LogEntry } from '../types';

  type Props = {
    logs: ReadonlyArray<LogEntry>;
  };

  const RECENT_LOG_LIMIT = 300;

  const { logs }: Props = $props();
  const recent = $derived(logs.slice(-RECENT_LOG_LIMIT));
</script>

<section class="panel logs-panel">
  <div class="panel-title">logs</div>
  <pre>{#each recent as log (log.index)}<span class="line"><span class="idx">{String(log.index).padStart(5, '0')}</span> {log.text}</span>
{:else}<span class="empty">waiting for logs</span>{/each}</pre>
</section>
