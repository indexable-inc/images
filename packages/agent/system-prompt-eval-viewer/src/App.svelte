<script lang="ts">
  import { onMount } from 'svelte';
  import type { Report } from './lib/types';
  import { pct, grade } from './lib/types';
  import Behaviors from './lib/Behaviors.svelte';
  import Rollout from './lib/Rollout.svelte';
  import sample from './sample.json';

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

  function expandAll(open: boolean) {
    document.querySelectorAll<HTMLDetailsElement>('details.rollout').forEach((d) => (d.open = open));
  }
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
  <header>
    <div class="brand">
      <span class="logo">◆</span>
      <h1>system&#8209;prompt evals</h1>
      {#if isSample}<span class="pill">sample</span>{/if}
    </div>
    <label class="loadbtn">
      load JSON
      <input
        type="file"
        accept="application/json,.json"
        onchange={(e) => {
          const f = (e.currentTarget as HTMLInputElement).files?.[0];
          if (f) load(f);
        }}
      />
    </label>
  </header>

  {#if !data}
    <p class="muted">loading…</p>
  {:else}
    <div class="meta">
      {#each meta as [k, v]}
        <span class="chip"><b>{k}</b> {v === null ? '—' : String(v)}</span>
      {/each}
    </div>

    <div class="cards">
      {#each evals as [name, ev]}
        <a class="card" href="#{name}">
          <div class="cn">{name}</div>
          <div class="cv {grade(ev.headline)}">{pct(ev.headline)}</div>
          {#if ev.longest_streak != null}<div class="cs">streak {ev.longest_streak}</div>{/if}
        </a>
      {/each}
    </div>

    <div class="bar">
      <button onclick={() => expandAll(true)}>expand all</button>
      <button onclick={() => expandAll(false)}>collapse all</button>
      <span class="hint">click a run for its full action timeline · click a behavior dot to jump to a run</span>
    </div>

    {#each evals as [name, ev], i}
      <section>
        <div class="sh" id={name}>
          <h2>{name}</h2>
          <span class="big {grade(ev.headline)}">{pct(ev.headline)}</span>
        </div>
        {#if ev.summary.cost}
          <div class="meta">
            {#if ev.summary.cost.mean_duration_s != null}<span class="chip"><b>mean</b> {Math.round(ev.summary.cost.mean_duration_s)}s</span>{/if}
            {#if ev.summary.cost.total_output_tokens != null}<span class="chip"><b>out</b> {ev.summary.cost.total_output_tokens.toLocaleString()} tok</span>{/if}
            {#if ev.summary.cost.total_cost_usd != null}<span class="chip"><b>cost</b> ${ev.summary.cost.total_cost_usd.toFixed(2)}</span>{/if}
            {#if ev.summary.sandbox != null}<span class="chip"><b>sandbox</b> {ev.summary.sandbox}</span>{/if}
          </div>
        {/if}
        <Behaviors {ev} eid={`e${i}`} />
        <h3>rollouts</h3>
        {#each ev.cases as c, j}
          <Rollout {c} anchor={`e${i}-${j}`} />
        {/each}
      </section>
    {/each}
  {/if}
</div>

{#if dragging}
  <div class="drop">drop an eval JSON to view it</div>
{/if}

<style>
  .wrap { max-width: 1040px; margin: 0 auto; padding: 28px 20px 120px; }
  header { display: flex; align-items: center; justify-content: space-between; margin-bottom: 14px; }
  .brand { display: flex; align-items: center; gap: 12px; }
  .logo { font-size: 22px; background: var(--grad); -webkit-background-clip: text; background-clip: text; color: transparent; }
  h1 { font-size: 20px; font-weight: 700; letter-spacing: -0.01em; margin: 0; }
  .pill { font-size: 11px; color: var(--accent); border: 1px solid color-mix(in oklab, var(--accent) 45%, transparent);
    border-radius: 999px; padding: 2px 9px; }
  .loadbtn { font-size: 13px; border: 1px solid var(--line); border-radius: 10px; padding: 7px 14px; cursor: pointer;
    background: var(--card); transition: border-color 0.15s; }
  .loadbtn:hover { border-color: var(--accent); }
  .loadbtn input { display: none; }
  .meta { display: flex; flex-wrap: wrap; gap: 6px; margin: 10px 0 16px; }
  .chip { font-size: 12px; background: var(--chip); border: 1px solid var(--line); border-radius: 8px; padding: 3px 9px; color: var(--dim); }
  .chip b { color: var(--text); font-weight: 600; }
  .cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: 12px; margin: 6px 0 22px; }
  .card { text-decoration: none; color: inherit; border: 1px solid var(--line); border-radius: 16px; padding: 16px 18px;
    background: var(--card); position: relative; overflow: hidden; transition: transform 0.12s, border-color 0.15s; }
  .card::before { content: ''; position: absolute; inset: 0 0 auto 0; height: 3px; background: var(--grad); opacity: 0.8; }
  .card:hover { transform: translateY(-2px); border-color: color-mix(in oklab, var(--accent) 40%, var(--line)); }
  .cn { font-size: 13px; color: var(--dim); }
  .cv { font-size: 34px; font-weight: 800; letter-spacing: -0.02em; }
  .cs { font-size: 12px; color: var(--dim); }
  .good { color: var(--good); } .warn { color: var(--warn); } .bad { color: var(--bad); }
  .bar { position: sticky; top: 0; z-index: 5; display: flex; align-items: center; gap: 8px;
    padding: 10px 0; margin-bottom: 8px; background: linear-gradient(var(--bg), color-mix(in oklab, var(--bg) 80%, transparent)); }
  .bar button { font: inherit; font-size: 12px; border: 1px solid var(--line); border-radius: 9px; padding: 5px 12px;
    background: var(--card); color: var(--text); cursor: pointer; }
  .bar button:hover { border-color: var(--accent); }
  .hint { color: var(--dim); font-size: 12px; }
  section { margin-top: 26px; }
  .sh { display: flex; align-items: baseline; justify-content: space-between; border-top: 1px solid var(--line); padding-top: 18px; }
  h2 { font-size: 17px; margin: 0; }
  .big { font-size: 26px; font-weight: 800; }
  h3 { font-size: 12px; color: var(--dim); text-transform: uppercase; letter-spacing: 0.08em; margin: 16px 0 6px; }
  .muted { color: var(--dim); }
  .drop { position: fixed; inset: 0; display: grid; place-items: center; font-size: 20px; font-weight: 600;
    background: color-mix(in oklab, var(--accent) 20%, color-mix(in oklab, var(--bg) 70%, transparent));
    border: 3px dashed var(--accent); z-index: 50; }
</style>
