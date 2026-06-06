<script lang="ts">
  import type { Message } from '$lib/types';
  import { localTime, absoluteTime } from '$lib/time';

  interface Props {
    message: Message;
  }

  let { message }: Props = $props();

  // Errors (kind 'error', or a legacy "turn failed:" system row) get a
  // contained, readable block instead of the centered small-caps divider used
  // for benign notices like a cancelled turn.
  let raw = $derived(message.text ?? '');
  let isError = $derived(message.kind === 'error' || raw.startsWith('turn failed:'));
  // Drop the redundant "turn failed:" prefix; the block carries its own label.
  let body = $derived(raw.replace(/^turn failed:\s*/, '').trim() || (message.kind ?? 'error'));
  // Long or multi-line errors collapse to a few lines until expanded.
  let collapsible = $derived(body.length > 220 || body.includes('\n'));
  let expanded = $state(false);
</script>

{#if isError}
  <div class="err" data-kind={message.kind} data-message-id={message.id}>
    <div class="err-head">
      <span class="err-tag">error</span>
      <span class="ts" title={absoluteTime(message.ts_ms)}>{localTime(message.ts_ms)}</span>
    </div>
    <div class="err-body" class:clamped={collapsible && !expanded}>{body}</div>
    {#if collapsible}
      <button class="err-more" onclick={() => (expanded = !expanded)}>
        {expanded ? 'show less' : 'show more'}
      </button>
    {/if}
  </div>
{:else}
  <div class="row" data-kind={message.kind} data-message-id={message.id}>
    <span class="line"></span>
    <span class="label" title={absoluteTime(message.ts_ms)}
      >{message.text ?? message.kind} <span class="ts">· {localTime(message.ts_ms)}</span></span
    >
    <span class="line"></span>
  </div>
{/if}

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 20px 0;
    color: var(--text-dim);
    font-size: 12px;
    scroll-margin-top: 32px;
  }
  .line {
    flex: 1;
    height: 1px;
    background: var(--border);
  }
  .label {
    font-variant: small-caps;
    letter-spacing: 0.04em;
    color: var(--text-muted);
  }
  .ts {
    margin-left: 4px;
    color: var(--text-dim);
    font-variant: normal;
    letter-spacing: 0;
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }

  .err {
    margin: 16px 0;
    padding: 9px 11px;
    border: 1px solid var(--border-hi);
    border-radius: var(--radius-sm);
    background: var(--bg-pill);
    font-size: 12px;
    scroll-margin-top: 32px;
  }
  .err-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    margin-bottom: 5px;
  }
  .err-tag {
    font-variant: small-caps;
    letter-spacing: 0.06em;
    font-weight: 600;
    color: var(--danger);
  }
  .err-head .ts {
    margin-left: 0;
  }
  .err-body {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    line-height: 1.5;
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 11.5px;
  }
  .err-body.clamped {
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .err-more {
    margin-top: 6px;
    padding: 0;
    border: 0;
    background: none;
    cursor: pointer;
    font: inherit;
    font-size: 11px;
    color: var(--text-dim);
  }
  .err-more:hover {
    color: var(--text);
  }
</style>
