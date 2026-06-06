<script lang="ts">
  import type { Message } from '$lib/types';
  import MarkdownBody from './MarkdownBody.svelte';
  import AnnotationFlag from './AnnotationFlag.svelte';
  import { localTime, absoluteTime } from '$lib/time';

  interface Props {
    serverId: string;
    message: Message;
  }

  let { serverId, message }: Props = $props();
</script>

<div class="row" data-message-id={message.id}>
  <MarkdownBody source={message.text ?? ''} />
  <div class="footer">
    <span class="ts" title={absoluteTime(message.ts_ms)}>{localTime(message.ts_ms)}</span>
    <AnnotationFlag {serverId} messageId={message.id} />
  </div>
</div>

<style>
  .row {
    margin: 10px 0 16px;
    max-width: 100%;
    color: var(--text);
    font-size: 12.5px;
    line-height: 1.55;
    scroll-margin-top: 32px;
  }
  .footer {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 2px;
  }
  .ts {
    color: var(--text-dim);
    font-size: 10.5px;
    font-variant-numeric: tabular-nums;
  }
</style>
