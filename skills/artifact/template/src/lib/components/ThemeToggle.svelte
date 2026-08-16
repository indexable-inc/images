<script lang="ts">
  let theme = $state(document.documentElement.dataset.theme ?? 'light');

  $effect(() => {
    document.documentElement.dataset.theme = theme;
  });
</script>

<button
  class="toggle"
  aria-label="toggle color theme"
  onclick={() => (theme = theme === 'dark' ? 'light' : 'dark')}
>
  {theme === 'dark' ? 'light' : 'dark'}
</button>

<style>
  .toggle {
    font: inherit;
    font-size: 0.8rem;
    color: var(--fg-muted);
    background: none;
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 0.25rem 0.6rem;
    cursor: pointer;
  }

  .toggle:hover {
    background: var(--bg-raised);
  }
</style>
