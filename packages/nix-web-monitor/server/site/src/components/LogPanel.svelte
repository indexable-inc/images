<script lang="ts">
  import { parseAnsi, type AnsiSegment } from '../lib/ansi';
  import type { LogEntry } from '../types';

  type Props = {
    logs: ReadonlyArray<LogEntry>;
  };

  const RECENT_LOG_LIMIT = 300;

  const { logs }: Props = $props();
  const recent = $derived(logs.slice(-RECENT_LOG_LIMIT));

  function segmentClass(segment: AnsiSegment): string {
    const classes: string[] = [];
    if (segment.fg !== null) classes.push(`fg-${segment.fg}`);
    if (segment.bg !== null) classes.push(`bg-${segment.bg}`);
    if (segment.bold) classes.push('bold');
    return classes.join(' ');
  }
</script>

<section class="panel logs-panel">
  <div class="panel-title">logs</div>
  <pre>{#each recent as log (log.index)}<span class="line"><span class="idx">{String(log.index).padStart(5, '0')}</span> {#each parseAnsi(log.text) as segment, segmentIndex (segmentIndex)}<span class={segmentClass(segment)}>{segment.text}</span>{/each}</span>
{:else}<span class="empty">waiting for logs</span>{/each}</pre>
</section>
