<script lang="ts">
  import type { Resource } from '$lib/types';
  let { resource }: { resource: Resource } = $props();
</script>

<article class="res-card {resource.status}">
  <header class="res-hdr">
    <span class="res-dot"></span>
    <span class="res-title">{resource.title}</span>
    <span class="res-kind">{resource.kind}</span>
  </header>
  <!-- agent-produced live render (a terminal screen, a widget); injected as-is -->
  <div class="res-body">{@html resource.html}</div>
</article>

<style>
  .res-card {
    margin: 0 0 9px;
    padding: 9px 11px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-left: 2px solid var(--line-2);
  }
  .res-card.live {
    border-left-color: var(--active);
  }
  .res-card.error {
    border-left-color: var(--err);
  }
  .res-hdr {
    display: flex;
    gap: 8px;
    align-items: center;
    margin: 0 0 7px;
  }
  .res-dot {
    flex: none;
    width: 6px;
    height: 6px;
    background: var(--active);
  }
  .res-card.error .res-dot {
    background: var(--err);
  }
  .res-title {
    overflow: hidden;
    color: var(--text);
    font-size: 12px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .res-kind {
    flex: none;
    margin-left: auto;
    color: var(--faint);
    font-size: 10px;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }
  .res-body {
    overflow: auto;
    max-height: 62vh;
  }
</style>
