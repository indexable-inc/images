<script lang="ts">
  import { untrack, type Snippet } from 'svelte';

  type Props = {
    title?: string;
    hint?: string;
    hintSnippet?: Snippet;
    collapsible?: boolean;
    initiallyCollapsed?: boolean;
    children: Snippet;
  };

  const {
    title,
    hint,
    hintSnippet,
    collapsible = true,
    initiallyCollapsed = false,
    children
  }: Props = $props();

  // untrack makes the "seed from props once at mount" intent explicit: after
  // this point `collapsed` is owned by the toggle button, not the props.
  let collapsed = $state(untrack(() => collapsible && initiallyCollapsed));

  function toggle() {
    if (!collapsible) return;
    collapsed = !collapsed;
  }
</script>

<div class="box">
  {#if title}
    {#if collapsible}
      <button class="title title-button" type="button" aria-expanded={!collapsed} onclick={toggle}>
        <span class="title-affordance" aria-hidden="true">{collapsed ? '[+]' : '[-]'}</span>
        <span>{title}</span>
      </button>
    {:else}
      <span class="title">{title}</span>
    {/if}
  {/if}
  {#if !collapsed}
    {#if hintSnippet}
      <span class="hint">{@render hintSnippet()}</span>
    {:else if hint}
      <span class="hint">{hint}</span>
    {/if}
    {@render children()}
  {:else}
    <span class="collapsed-indicator" aria-hidden="true">...</span>
  {/if}
</div>

<style>
  .box {
    position: relative;
    border: 1px solid var(--ix-border);
    border-radius: 4px;
    padding: 1lh 2ch;
    display: flex;
    flex-direction: column;
    gap: 0.5lh;
    margin-top: 0.6lh;
    background: var(--ix-bg);
    min-width: 0;
  }

  .title {
    position: absolute;
    top: calc(-0.5lh);
    left: 1.5ch;
    padding: 0 1ch;
    background: var(--ix-bg);
    color: var(--ix-ink-muted);
    line-height: 1lh;
  }

  .title-button {
    appearance: none;
    border: none;
    cursor: pointer;
    display: inline-flex;
    gap: 1ch;
    font: inherit;
    color: inherit;
  }

  .title-button:hover {
    color: var(--ix-ink);
  }

  .title-affordance {
    color: var(--ix-ink-faint);
  }

  .hint {
    position: absolute;
    top: calc(-0.5lh);
    right: 1.5ch;
    padding: 0 1ch;
    background: var(--ix-bg);
    color: var(--ix-ink-faint);
    line-height: 1lh;
  }

  .collapsed-indicator {
    color: var(--ix-ink-faint);
    line-height: 1lh;
  }
</style>
