<script lang="ts">
  // New chat / search entry buttons at the top of the sidebar.

  import IconNewChat from '~icons/ph/pencil-simple-line';
  import IconSearch from '~icons/ph/magnifying-glass';

  interface Props {
    searching: boolean;
    onNewChat: (e?: MouseEvent) => void;
    onToggleSearch: () => void;
  }

  let { searching, onNewChat, onToggleSearch }: Props = $props();
</script>

<nav class="topnav">
  <button class="nav-item" onclick={(e) => onNewChat(e)}>
    <IconNewChat class="nav-icon" />
    <span>New chat</span>
    <kbd class="nav-kbd" aria-hidden="true">⌘N</kbd>
  </button>
  <button
    class="nav-item"
    class:active={searching}
    onclick={onToggleSearch}
  >
    <IconSearch class="nav-icon" />
    <span>Search</span>
  </button>
</nav>

<style>
  .topnav {
    display: flex;
    flex-direction: column;
    padding: 0 12px 10px;
    gap: 2px;
  }
  .nav-item {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 30px;
    padding: 5px 10px;
    border-radius: var(--radius-sm);
    color: var(--text);
    font-size: 13px;
    text-align: left;
    line-height: 1.4;
    transition: background 0.1s ease;
  }
  .nav-item :global(.nav-icon) {
    width: 15px;
    height: 15px;
    color: var(--text-muted);
    flex-shrink: 0;
  }
  .nav-item:hover {
    background: var(--bg-hover);
    color: var(--text-strong);
  }
  .nav-item:hover :global(.nav-icon) {
    color: var(--text-strong);
  }
  .nav-item.active {
    background: var(--bg-active);
    color: var(--text-strong);
  }
  .nav-kbd {
    margin-left: auto;
    color: var(--text-dim);
    font-size: 10.5px;
    font-family: var(--font-sans);
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--bg-pill);
    border: 1px solid var(--border);
  }
</style>
