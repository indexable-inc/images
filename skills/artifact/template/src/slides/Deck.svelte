<script lang="ts">
  let i = $state(0);
  const N = 3;

  // Deck revision. Bump REV and add an entry here on every content edit;
  // dots ring until the slide is viewed, and the current slide shows the note.
  const REV = 1;
  const changes: Record<number, { rev: number; note: string }> = {
    // 1: { rev: 2, note: 'describe what changed on slide index 1' }
  };
  function loadSeen(): Record<number, number> {
    try {
      return JSON.parse(localStorage.getItem('artifact-deck-seen') ?? '{}');
    } catch {
      return {};
    }
  }
  let seen = $state(loadSeen());
  $effect(() => {
    const rev = changes[i]?.rev ?? 1;
    if ((seen[i] ?? 1) < rev) {
      seen[i] = rev;
      localStorage.setItem('artifact-deck-seen', JSON.stringify(seen));
    }
  });

  function go(d: number) {
    i = Math.min(N - 1, Math.max(0, i + d));
  }
  function onkey(e: KeyboardEvent) {
    if (e.key === 'ArrowRight' || (e.key === ' ' && !e.shiftKey)) { go(1); e.preventDefault(); }
    else if (e.key === 'ArrowLeft' || (e.key === ' ' && e.shiftKey)) { go(-1); e.preventDefault(); }
    else if (e.key === 'Home') { i = 0; }
    else if (e.key === 'End') { i = N - 1; }
  }
</script>

<svelte:window onkeydown={onkey} />

{#snippet box(label: string, sub: string)}
  <div class="bx"><b>{label}</b><span>{sub}</span></div>
{/snippet}

{#snippet s0()}
  <h1>Deck title</h1>
  <p class="sub">one line that says what this deck is about</p>
  <p class="foot">← → to navigate</p>
{/snippet}

{#snippet s1()}
  <h2>structure, in three boxes</h2>
  <div class="flow">
    {@render box('input', 'what comes in')}
    <i>→</i>
    {@render box('transform', 'what happens to it')}
    <i>→</i>
    {@render box('output', 'what comes out')}
  </div>
  <p class="cap">the <code>box</code> snippet renders a labeled box; chain
    calls with an arrow inside a <code>.flow</code> row to sketch a pipeline</p>
{/snippet}

{#snippet s2()}
  <h2>that's the deck</h2>
  <p class="sub">replace these three slides with real content</p>
  <p class="foot">bump REV and add a <code>changes</code> entry on every edit</p>
{/snippet}

<main>
  {@render [s0, s1, s2][i]()}
</main>

<footer>
  {#if changes[i] && changes[i].rev === REV}
    <p class="changebar">updated — {changes[i].note}</p>
  {/if}
  <div class="controls">
    <button class="nav" onclick={() => go(-1)} disabled={i === 0}>←</button>
    <div class="dots">
      {#each Array(N) as _, d}
        <button
          class="dot"
          class:on={d === i}
          class:fresh={(changes[d]?.rev ?? 1) > (seen[d] ?? 1)}
          onclick={() => (i = d)}
          aria-label={'slide ' + (d + 1)}
        ></button>
      {/each}
    </div>
    <button class="nav" onclick={() => go(1)} disabled={i === N - 1}>→</button>
    <span class="count">{i + 1}/{N}</span>
  </div>
</footer>

<style>
  main {
    min-height: calc(100vh - 96px);
    display: flex; flex-direction: column; justify-content: center;
    max-width: 880px; margin: 0 auto; padding: 24px 40px;
  }
  footer {
    min-height: 64px; padding: 8px 0 14px; display: flex; flex-direction: column;
    align-items: center; justify-content: center; gap: 8px; color: var(--fg-muted);
  }
  .controls { display: flex; align-items: center; gap: 16px; }
  .changebar {
    margin: 0; font-size: 13px; color: var(--accent);
    background: var(--accent-soft); border-radius: 4px; padding: 3px 10px;
  }
  h1 { font-size: clamp(40px, 7vw, 72px); margin: 0; letter-spacing: -0.02em; }
  h2 { font-size: clamp(26px, 4vw, 40px); margin: 0 0 28px; letter-spacing: -0.01em; }
  .sub { font-size: clamp(18px, 2.5vw, 26px); color: var(--fg-muted); margin: 12px 0 0; }
  .foot { margin-top: 48px; color: var(--fg-muted); font-size: 15px; line-height: 1.7; }
  .cap { color: var(--fg-muted); font-size: 15px; margin-top: 20px; }
  .flow { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
  .flow i { color: var(--fg-muted); font-style: normal; font-size: 14px; }
  .bx {
    border: 1px solid var(--fg-muted); border-radius: 6px; padding: 10px 14px;
    display: flex; flex-direction: column; gap: 2px; background: var(--bg-raised);
  }
  .bx b { font-size: 16px; }
  .bx span { font-size: 13px; color: var(--fg-muted); }
  .nav {
    background: none; border: 1px solid var(--fg-muted); border-radius: 4px;
    color: var(--fg); font-size: 16px; padding: 4px 14px; cursor: pointer;
  }
  .nav:disabled { opacity: 0.3; cursor: default; }
  .dots { display: flex; gap: 8px; }
  .dot {
    width: 8px; height: 8px; border-radius: 50%; border: none; padding: 0;
    background: var(--fg-muted); opacity: 0.4; cursor: pointer;
  }
  .dot.on { background: var(--accent); opacity: 1; }
  .dot.fresh { opacity: 1; box-shadow: 0 0 0 2px var(--accent); background: var(--bg); }
  .dot.fresh.on { background: var(--accent); }
  .count { font-size: 13px; font-variant-numeric: tabular-nums; }
  code { font-family: ui-monospace, monospace; font-size: 0.9em; }
</style>
