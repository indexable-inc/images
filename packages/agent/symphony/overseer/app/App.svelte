<script lang="ts">
  import type { Data, AgentJudgment } from "./types";
  import AttentionCard from "./lib/AttentionCard.svelte";
  import AgentCard from "./lib/AgentCard.svelte";
  import StateDot from "./lib/StateDot.svelte";
  import Gauge from "./lib/Gauge.svelte";
  import RunChip from "./lib/RunChip.svelte";

  // The tick script splices data.json into the #overseer-data script tag.
  const data: Data = JSON.parse(document.getElementById("overseer-data")!.textContent!);
  const { report, history } = data;

  // Broken-first: stuck agents get full cards, waiting a compact list,
  // progressing a dot grid, idle just a count.
  // Unknown states (LLM drift off the enum) bucket as waiting so an agent
  // never silently vanishes from the page.
  const state = (a: AgentJudgment) =>
    ["progressing", "waiting", "stuck", "idle"].includes(a.state) ? a.state : "waiting";
  const stuck = $derived(report.agents.filter((a) => state(a) === "stuck"));
  const waiting = $derived(report.agents.filter((a) => state(a) === "waiting"));
  const progressing = $derived(report.agents.filter((a) => state(a) === "progressing"));
  const idle = $derived(report.agents.filter((a) => state(a) === "idle"));

  const badRuns = $derived(data.runs.filter((r) => r.status === "failed" || r.status === "running"));
  const okRuns = $derived(data.runs.length - badRuns.length);

  const fixes = $derived(report.attention.filter((a) => a.severity === "fix"));
  const updated = $derived(new Date(data.generated_at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }));
</script>

<main>
  <header>
    <h1>overseer</h1>
    <span class="verdict" class:calm={fixes.length === 0 && stuck.length === 0}>
      {#if fixes.length > 0}{fixes.length} need{fixes.length === 1 ? "s" : ""} you
      {:else if stuck.length > 0}{stuck.length} stuck
      {:else}all clear{/if}
    </span>
    <span class="meta">{updated}</span>
  </header>

  <p class="digest">{report.digest}</p>

  {#if report.attention.length > 0}
    <section class="attention">
      {#each report.attention as item}
        <AttentionCard {item} />
      {/each}
    </section>
  {/if}

  <section class="gauges">
    <Gauge points={history.map((h) => h.cpu ?? 0)} label="cpu" unit="%" warnAt={85} />
    <Gauge points={history.map((h) => h.mem ?? 0)} label="memory" unit="%" warnAt={90} />
    <Gauge points={history.map((h) => h.stuck)} label="stuck" warnAt={1} />
    <Gauge points={history.map((h) => h.sessions)} label="sessions" />
  </section>

  {#if stuck.length > 0}
    <section>
      <h2>stuck</h2>
      {#each stuck as agent}<AgentCard {agent} />{/each}
    </section>
  {/if}

  {#if waiting.length > 0}
    <section>
      <h2>waiting</h2>
      {#each waiting as agent}<AgentCard {agent} />{/each}
    </section>
  {/if}

  {#if progressing.length > 0}
    <section>
      <h2>progressing</h2>
      <div class="grid">
        {#each progressing as agent}
          <details class="mini">
            <summary><StateDot state={agent.state} /><span>{agent.label}</span></summary>
            <p>{agent.doing}</p>
            <p class="why">{agent.why}</p>
          </details>
        {/each}
      </div>
      {#if idle.length > 0}<p class="idle">+ {idle.length} idle</p>{/if}
    </section>
  {/if}

  {#if badRuns.length > 0 || okRuns > 0}
    <section>
      <h2>symphony</h2>
      <div class="chips">
        {#each badRuns as run}<RunChip {run} />{/each}
        {#if okRuns > 0}<span class="okruns">{okRuns} succeeded</span>{/if}
      </div>
    </section>
  {/if}

  <details class="notes">
    <summary>overseer notes</summary>
    <pre>{data.notes}</pre>
  </details>
</main>

<style>
  :global(body) {
    margin: 0;
    font: 15px/1.55 -apple-system, "Helvetica Neue", sans-serif;
    color: var(--fg);
    background: var(--bg);
    --fg: #1a1a1a; --bg: #fcfcfa; --muted: #8a8a86; --border: #e6e6e2;
    --mono: ui-monospace, "SF Mono", monospace;
  }
  @media (prefers-color-scheme: dark) {
    :global(body) { --fg: #e6e6e2; --bg: #161615; --muted: #85857f; --border: #2c2c2a; }
  }
  main { max-width: 46rem; margin: 3rem auto; padding: 0 1rem; }
  header { display: flex; align-items: baseline; gap: 0.9rem; }
  h1 { font-size: 1rem; font-weight: 600; margin: 0; }
  .verdict { font-weight: 650; color: #e05252; }
  .verdict.calm { color: #3fb96f; }
  .meta { color: var(--muted); font-size: 0.8rem; margin-left: auto; font-variant-numeric: tabular-nums; }
  .digest { color: var(--muted); margin: 0.5rem 0 0; }
  .attention { display: grid; gap: 0.6rem; margin-top: 1.1rem; }
  .gauges { display: flex; gap: 1.4rem; margin-top: 1.6rem; flex-wrap: wrap; }
  h2 { font-size: 0.72rem; font-weight: 600; color: var(--muted); text-transform: uppercase;
       letter-spacing: 0.06em; margin: 1.8rem 0 0.4rem; }
  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(13rem, 1fr)); gap: 0.15rem 1rem; }
  .mini summary { display: flex; align-items: baseline; gap: 0.5rem; padding: 0.25rem 0; cursor: pointer; list-style: none; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .mini summary::-webkit-details-marker { display: none; }
  .mini p { margin: 0.1rem 0 0.4rem 1rem; font-size: 0.85rem; }
  .mini .why { color: var(--muted); }
  .idle { color: var(--muted); font-size: 0.8rem; }
  .chips { display: flex; gap: 0.5rem; flex-wrap: wrap; align-items: baseline; }
  .okruns { color: var(--muted); font-size: 0.8rem; }
  .notes { margin: 2.2rem 0 3rem; color: var(--muted); }
  .notes summary { cursor: pointer; font-size: 0.8rem; }
  .notes pre { font-size: 12px; white-space: pre-wrap; font-family: var(--mono); }
</style>
