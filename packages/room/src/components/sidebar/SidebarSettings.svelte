<script lang="ts">
  // Bottom bay of the sidebar: a single trigger button for the
  // SettingsModal overlay. The form itself lives in
  // components/SettingsModal.svelte — inline expansion under a narrow
  // sidebar truncates URLs and bumps the chat list around on every
  // open. The button stays here so the entry point is still anchored
  // to the sidebar chrome operators reach for.

  import { settingsOpen, toggleSettings } from '$lib/ui';
  import IconSettings from '~icons/ph/sliders-horizontal';

  let showSettings = $state(false);
  $effect(() => settingsOpen.subscribe((v) => (showSettings = v)));
</script>

<div class="settings-bay">
  <button class="settings" onclick={toggleSettings} class:active={showSettings}>
    <IconSettings class="nav-icon" />
    <span>Settings</span>
    <kbd class="footer-kbd" aria-hidden="true">⌘,</kbd>
  </button>
</div>

<style>
  .settings-bay {
    border-top: 1px solid var(--border);
    background: transparent;
    padding: 6px 12px;
  }
  .settings {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    min-height: 30px;
    padding: 5px 10px;
    border-radius: var(--radius-sm);
    color: var(--text-muted);
    font-size: 13px;
    text-align: left;
    cursor: pointer;
  }
  .settings :global(.nav-icon) {
    width: 16px;
    height: 16px;
    color: var(--text-muted);
    stroke-width: 1.75;
  }
  .settings:hover,
  .settings.active {
    color: var(--text-strong);
    background: var(--bg-hover);
  }
  .settings:hover :global(.nav-icon),
  .settings.active :global(.nav-icon) {
    color: var(--text-strong);
  }
  .footer-kbd {
    margin-left: auto;
    color: var(--text-dim);
    font-size: 10.5px;
    font-family: var(--font-sans);
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--bg-pill);
    border: 1px solid var(--border);
  }
  .settings:hover .footer-kbd,
  .settings.active .footer-kbd {
    color: var(--text-muted);
  }
</style>
