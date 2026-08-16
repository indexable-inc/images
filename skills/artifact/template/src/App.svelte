<script lang="ts">
  import DiffView from './lib/components/DiffView.svelte';
  import ThemeToggle from './lib/components/ThemeToggle.svelte';
  import VersionPicker from './lib/components/VersionPicker.svelte';
  import { versions } from './lib/versions';

  let selected = $state(versions.length - 1);
  let diffMode = $state(false);
  let base = $state(Math.max(0, versions.length - 2));

  const current = $derived(versions[selected]);
  const Page = $derived(current.component);
</script>

<div class="page">
  <header class="chrome">
    <VersionPicker {versions} bind:selected bind:diffMode bind:base />
    <ThemeToggle />
  </header>

  {#if diffMode && versions.length > 1}
    <DiffView base={versions[base]} target={current} />
  {:else}
    <main class="prose">
      <Page />
    </main>
  {/if}
</div>

<style>
  .chrome {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    margin-bottom: 2.5rem;
  }
</style>
