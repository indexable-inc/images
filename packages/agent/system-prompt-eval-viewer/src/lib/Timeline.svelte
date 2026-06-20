<script lang="ts">
  import type { Step } from './types';
  let { steps, transcript }: { steps?: Step[]; transcript?: string } = $props();

  const icon: Record<string, string> = {
    text: '\u{1F4AC}',
    thinking: '\u{1F914}',
    tool_use: '\u{1F527}',
    tool_result: '\u{2192}',
    final: '\u{2714}',
  };
</script>

{#if steps && steps.length}
  <div class="timeline">
    <div class="count">{steps.length} steps</div>
    {#each steps as s}
      <div class="step {s.kind} {s.is_error ? 'err' : ''}">
        <div class="rail"><span class="ico">{icon[s.kind] ?? '\u{2022}'}</span></div>
        <div class="body">
          {#if s.kind === 'tool_use'}
            <div class="head">tool <b>{s.name}</b></div>
            <pre class="code">{s.input}</pre>
          {:else if s.kind === 'tool_result'}
            <div class="head">{s.is_error ? 'tool result · error' : 'tool result'}</div>
            <pre class="out">{s.text}</pre>
          {:else if s.kind === 'thinking'}
            <details><summary>thinking</summary><pre class="think">{s.text}</pre></details>
          {:else if s.kind === 'final'}
            <div class="head">final answer</div>
            <div class="prose final">{s.text}</div>
          {:else}
            <div class="head">assistant</div>
            <div class="prose">{s.text}</div>
          {/if}
        </div>
      </div>
    {/each}
  </div>
{:else if transcript}
  <pre class="out">{transcript}</pre>
{:else}
  <p class="muted">no transcript captured</p>
{/if}

<style>
  .timeline { position: relative; padding-left: 4px; }
  .count { font-size: 11px; color: var(--dim); text-transform: uppercase; letter-spacing: 0.08em; margin: 2px 0 10px; }
  .step { display: grid; grid-template-columns: 26px 1fr; gap: 10px; margin: 10px 0; }
  .rail { display: flex; justify-content: center; }
  .ico {
    width: 26px; height: 26px; border-radius: 8px; display: grid; place-items: center;
    background: var(--chip); font-size: 13px; border: 1px solid var(--line);
  }
  .step.tool_use .ico { background: color-mix(in oklab, var(--accent) 22%, transparent); }
  .step.final .ico { background: color-mix(in oklab, var(--good) 26%, transparent); }
  .step.err .ico { background: color-mix(in oklab, var(--bad) 26%, transparent); }
  .head { font-size: 12px; color: var(--dim); margin-bottom: 3px; }
  .head b { color: var(--accent); font-weight: 600; }
  .prose { white-space: pre-wrap; line-height: 1.5; }
  .prose.final {
    background: color-mix(in oklab, var(--good) 12%, transparent);
    border: 1px solid color-mix(in oklab, var(--good) 35%, transparent);
    border-radius: 10px; padding: 10px 12px;
  }
  pre { margin: 4px 0 0; white-space: pre-wrap; word-break: break-word; border-radius: 10px;
    padding: 10px 12px; font: 12px/1.5 var(--mono); max-height: 380px; overflow: auto; }
  .code { background: color-mix(in oklab, var(--accent) 10%, var(--panel)); border: 1px solid var(--line); }
  .out { background: var(--panel); border: 1px solid var(--line); color: var(--text); }
  .step.err .out { border-color: color-mix(in oklab, var(--bad) 50%, var(--line)); }
  .think { background: var(--panel); border: 1px dashed var(--line); color: var(--dim); }
  details summary { cursor: pointer; color: var(--dim); font-size: 12px; }
  .muted { color: var(--dim); }
</style>
