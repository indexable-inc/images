<script lang="ts">
  import { onMount } from 'svelte';
  import type { Report } from './lib/types';
  import sample from './sample.json';
  import Header from './lib/Header.svelte';
  import MetaBar from './lib/MetaBar.svelte';
  import ScoreCard from './lib/ScoreCard.svelte';
  import Toolbar from './lib/Toolbar.svelte';
  import EvalSection from './lib/EvalSection.svelte';
  import DropZone from './lib/DropZone.svelte';

  let data = $state<Report | null>(null);
  let dragging = $state(false);
  let isSample = $state(false);

  onMount(async () => {
    // The nix wrapper drops the run's JSON next to index.html as data.json.
    try {
      const r = await fetch('./data.json', { cache: 'no-store' });
      if (r.ok) {
        data = await r.json();
        return;
      }
    } catch {
      /* no companion file; fall through to the bundled sample */
    }
    data = sample as unknown as Report;
    isSample = true;
  });

  function load(file: File) {
    const fr = new FileReader();
    fr.onload = () => {
      try {
        data = JSON.parse(String(fr.result)) as Report;
        isSample = false;
      } catch {
        alert('not a valid eval JSON file');
      }
    };
    fr.readAsText(file);
  }

  function onDrop(e: DragEvent) {
    e.preventDefault();
    dragging = false;
    const f = e.dataTransfer?.files?.[0];
    if (f) load(f);
  }

  const evals = $derived(data ? Object.entries(data.evals) : []);
  const meta = $derived(data ? Object.entries(data.metadata) : []);
</script>

<svelte:window
  ondragover={(e) => {
    e.preventDefault();
    dragging = true;
  }}
  ondragleave={() => (dragging = false)}
  ondrop={onDrop}
/>

<div class="wrap">
  <Header {isSample} onpick={load} />

  {#if !data}
    <p class="muted">loading…</p>
  {:else}
    <MetaBar entries={meta} />
    <div class="cards">
      {#each evals as [name, ev]}
        <ScoreCard {name} {ev} />
      {/each}
    </div>
    <Toolbar />
    {#each evals as [name, ev], i}
      <EvalSection {name} {ev} idx={i} />
    {/each}
  {/if}
</div>

<DropZone active={dragging} />

<style>
  .wrap { max-width: 1040px; margin: 0 auto; padding: 28px 20px 120px; }
  .cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: 10px; margin: 6px 0 26px; }
  .muted { color: var(--dim); }
</style>
