<script lang="ts">
  import type { SessionMeta } from '$lib/types';

  interface Props {
    sessions: SessionMeta[];
    activeId: string | null;
    onselect: (id: string) => void;
    oncreate: () => void;
    onrename: (id: string, name: string) => void;
    onclose: (id: string) => void;
  }

  const { sessions, activeId, onselect, oncreate, onrename, onclose }: Props =
    $props();

  let editingId = $state<string | null>(null);
  let editValue = $state('');

  function beginRename(s: SessionMeta): void {
    editingId = s.id;
    editValue = s.name;
  }

  function commitRename(): void {
    if (editingId === null) {
      return;
    }
    const id = editingId;
    const name = editValue.trim();
    editingId = null;
    const current = sessions.find((s) => s.id === id);
    if (name.length > 0 && current !== undefined && name !== current.name) {
      onrename(id, name);
    }
  }

  function cancelRename(): void {
    editingId = null;
  }

  function focusSelect(node: HTMLInputElement): void {
    node.focus();
    node.select();
  }

  function createdTooltip(s: SessionMeta): string {
    return `created ${new Date(s.created_at_ms).toLocaleString()}`;
  }
</script>

<nav class="tabbar">
  <div class="tabs" role="tablist" aria-label="sessions" aria-orientation="vertical">
    {#each sessions as s (s.id)}
      <div
        class="tab"
        class:active={s.id === activeId}
        role="tab"
        aria-selected={s.id === activeId}
        tabindex="0"
        title={createdTooltip(s)}
        onclick={() => {
          onselect(s.id);
        }}
        onkeydown={(ev) => {
          if (ev.key === 'Enter') {
            onselect(s.id);
          }
        }}
        ondblclick={() => {
          beginRename(s);
        }}
      >
        {#if editingId === s.id}
          <input
            class="rename"
            bind:value={editValue}
            use:focusSelect
            onblur={commitRename}
            onkeydown={(ev) => {
              if (ev.key === 'Enter') {
                commitRename();
              } else if (ev.key === 'Escape') {
                cancelRename();
              }
              ev.stopPropagation();
            }}
            onclick={(ev) => {
              ev.stopPropagation();
            }}
            ondblclick={(ev) => {
              ev.stopPropagation();
            }}
          />
        {:else}
          <span class="name">{s.name}</span>
          <button
            class="close"
            aria-label="close session"
            onclick={(ev) => {
              ev.stopPropagation();
              onclose(s.id);
            }}>x</button
          >
        {/if}
      </div>
    {/each}
  </div>
  <button class="create" aria-label="new session" onclick={oncreate}>+</button>
</nav>

<style>
  .tabbar {
    width: 200px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    background: #171717;
    border-right: 1px solid #262626;
    padding: 8px 6px;
    gap: 6px;
    overflow-y: auto;
  }

  .tabs {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .tab {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 8px;
    border-radius: 5px;
    color: #aaa;
    cursor: pointer;
    user-select: none;
  }

  .tab:hover {
    background: #222;
  }

  .tab.active {
    background: #2b2b2b;
    color: #eee;
  }

  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .close {
    visibility: hidden;
    background: none;
    border: none;
    color: #777;
    cursor: pointer;
    padding: 0 3px;
    border-radius: 3px;
    line-height: 1;
  }

  .tab:hover .close {
    visibility: visible;
  }

  .close:hover {
    color: #ddd;
    background: #333;
  }

  .rename {
    flex: 1;
    min-width: 0;
    background: #111;
    color: #eee;
    border: 1px solid #444;
    border-radius: 3px;
    padding: 2px 4px;
    font: inherit;
  }

  .create {
    background: none;
    border: 1px dashed #333;
    border-radius: 5px;
    color: #888;
    cursor: pointer;
    padding: 4px 0;
  }

  .create:hover {
    color: #ddd;
    border-color: #555;
  }
</style>
