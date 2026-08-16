<script lang="ts">
  import type { Version } from '../versions';

  let {
    versions,
    selected = $bindable(),
    diffMode = $bindable(),
    base = $bindable()
  }: {
    versions: Version[];
    selected: number;
    diffMode: boolean;
    base: number;
  } = $props();

  // Keep the diff meaningful: never compare a version against itself.
  // Default base is the previous version; v0 falls back to comparing
  // forward against v1.
  function normalizeBase() {
    if (base !== selected) return;
    base = selected > 0 ? selected - 1 : Math.min(1, versions.length - 1);
  }

  function pick(index: number) {
    selected = index;
    normalizeBase();
  }
</script>

<nav class="picker">
  {#each versions as version, index (version.id)}
    <button
      class="tab"
      class:active={index === selected && !diffMode}
      title={version.note ?? version.title}
      onclick={() => {
        diffMode = false;
        pick(index);
      }}
    >
      {version.id}
    </button>
  {/each}

  {#if versions.length > 1}
    <span class="rule"></span>
    <button
      class="tab"
      class:active={diffMode}
      onclick={() => {
        diffMode = !diffMode;
        if (diffMode) normalizeBase();
      }}
    >
      diff
    </button>
    {#if diffMode}
      <select bind:value={base} aria-label="diff base">
        {#each versions as version, index (version.id)}
          {#if index !== selected}
            <option value={index}>{version.id}</option>
          {/if}
        {/each}
      </select>
      <span class="arrow">&rarr;</span>
      <select bind:value={selected} aria-label="diff target">
        {#each versions as version, index (version.id)}
          {#if index !== base}
            <option value={index}>{version.id}</option>
          {/if}
        {/each}
      </select>
    {/if}
  {/if}
</nav>

<style>
  .picker {
    display: flex;
    align-items: center;
    gap: 0.25rem;
  }

  .tab {
    font: inherit;
    font-size: 0.85rem;
    font-weight: 500;
    color: var(--fg-muted);
    background: none;
    border: 0;
    border-radius: 6px;
    padding: 0.3rem 0.6rem;
    cursor: pointer;
  }

  .tab:hover {
    background: var(--bg-raised);
  }

  .tab.active {
    color: var(--accent);
    background: var(--accent-soft);
  }

  .rule {
    width: 1px;
    height: 1.1rem;
    background: var(--border);
    margin: 0 0.35rem;
  }

  .arrow {
    color: var(--fg-muted);
    font-size: 0.85rem;
  }

  select {
    font: inherit;
    font-size: 0.85rem;
    color: var(--fg);
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 0.2rem 0.4rem;
  }
</style>
