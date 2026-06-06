<script lang="ts">
  import type { Message } from '$lib/types';
  import { localTime, absoluteTime } from '$lib/time';

  interface Props {
    message: Message;
  }

  let { message }: Props = $props();

  let images = $derived(message.images ?? []);
  let hasText = $derived(!!message.text && message.text.length > 0);
</script>

<div class="row" data-message-id={message.id}>
  {#if images.length > 0}
    <div class="images" class:single={images.length === 1}>
      {#each images as src, i (i)}
        <a class="thumb" href={src} target="_blank" rel="noopener noreferrer">
          <img {src} alt={`attachment ${i + 1}`} />
        </a>
      {/each}
    </div>
  {/if}
  {#if hasText}
    <div class="bubble">{message.text}</div>
  {/if}
  <span class="ts" title={absoluteTime(message.ts_ms)}>{localTime(message.ts_ms)}</span>
</div>

<style>
  .row {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    margin: 12px 0;
    scroll-margin-top: 32px;
  }
  .bubble {
    max-width: 78%;
    background: var(--bg-pill);
    border-radius: 14px;
    padding: 7px 12px;
    color: var(--text-strong);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    line-height: 1.5;
    font-size: 12.5px;
  }
  /* Image attachments. Multi-image grid wraps to keep big screens
     readable; a single image gets to stretch up to the bubble width
     cap so a screenshot is legible without click-through. */
  .images {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    justify-content: flex-end;
    max-width: 78%;
    margin-bottom: 4px;
  }
  .thumb {
    display: inline-flex;
    border-radius: 10px;
    overflow: hidden;
    background: var(--bg-pill);
    line-height: 0;
  }
  .thumb img {
    max-height: 220px;
    max-width: 100%;
    object-fit: contain;
    display: block;
  }
  .images.single .thumb img {
    max-height: 340px;
  }
  .ts {
    margin-top: 3px;
    margin-right: 4px;
    color: var(--text-dim);
    font-size: 10.5px;
    font-variant-numeric: tabular-nums;
  }
</style>
