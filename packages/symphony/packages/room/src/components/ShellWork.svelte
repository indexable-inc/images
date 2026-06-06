<script lang="ts">
  // Starship-style render for the codex `shell` tool.
  //
  // Codex always ships {command, cwd} in tool_input, so we lean into
  // that with a two-line prompt:
  //   <dir>
  //   ❯ <command>
  //   <output>
  //
  // The output sits directly below the prompt with no expand toggle —
  // a shell row reads as a captured terminal session. Output is
  // highlighted with shiki's `ansi` grammar so ANSI escape sequences
  // from real CLIs (git, npm, cargo, ls --color, etc.) come through
  // as colored spans instead of raw `[…` bytes. The command is
  // highlighted with bash grammar to match.
  //
  // The codex shell wrapper (`<path>/zsh -lc <cmd>` / `bash -c <cmd>`)
  // is stripped on the way in so the prompt reads as the inner
  // command the agent actually meant to run.

  import type { Message } from '$lib/types';
  import AnnotationFlag from './AnnotationFlag.svelte';
  import { highlightInline, highlighterReady } from '$lib/markdown';
  import { errorMessage, isRunning, strField, toolDisplay, toolStatus } from '$lib/toolWork';

  interface Props {
    serverId: string;
    message: Message;
  }

  let { serverId, message }: Props = $props();

  let shikiReady = $state(false);
  const unsub = highlighterReady.subscribe((v) => (shikiReady = v));
  $effect(() => () => unsub());

  function highlight(code: string, lang: string): string {
    void shikiReady;
    return highlightInline(code, lang);
  }

  // Codex's shell tool wraps user commands in a login shell:
  //   `<path>/zsh -lc '<cmd>'` or `bash -c "<cmd>"`.
  // The wrapper is implementation detail; strip it for display so
  // the prompt reads as the inner command. The quoted form is the
  // codex default, so capture inside the matched outer quotes
  // directly — otherwise the unwrap would leave a literal `'` /
  // `"` at both ends and the prompt would render as `'git pull'`
  // instead of `git pull`. Falls back to greedy unquoted capture
  // for any other shape, and to the raw string when nothing
  // matches at all.
  function unwrapShellCommand(raw: string): string {
    const quoted = raw.match(/^(?:\S*\/)?(?:bash|zsh|sh)\s+-l?c\s+(['"])([\s\S]*)\1\s*$/);
    if (quoted) return quoted[2]!;
    const bare = raw.match(/^(?:\S*\/)?(?:bash|zsh|sh)\s+-l?c\s+(.+)$/s);
    return bare ? bare[1]! : raw;
  }

  // Final directory segment, Starship-style. `/Users/.../symphony`
  // reads as just `symphony`.
  function cwdLabel(cwd: string): string {
    const trimmed = cwd.replace(/\/+$/, '');
    const base = trimmed.split('/').filter(Boolean).pop();
    return base ?? trimmed;
  }

  let rawCmd = $derived(strField(message.tool_input, 'command') ?? '');
  let cmd = $derived(unwrapShellCommand(rawCmd));
  let cwd = $derived(strField(message.tool_input, 'cwd'));
  let out = $derived(toolDisplay(message));
  let running = $derived(isRunning(message));
  let err = $derived(errorMessage(message));
  let cancelled = $derived(toolStatus(message) === 'cancelled');
  // Non-zero exit shows as a small badge next to the prompt. Codex
  // carries it on the typed result for both ok and error outcomes.
  let exitCode = $derived(
    message.result && 'exit_code' in message.result ? message.result.exit_code : null
  );
</script>

<div class="shell-step annotatable" data-message-id={message.id}>
  <AnnotationFlag {serverId} messageId={message.id} />
  {#if cwd}
    <div class="ctx">
      <span class="ctx-dir">{cwdLabel(cwd)}</span>
    </div>
  {/if}
  <div class="line">
    <span class="prompt" aria-hidden="true">❯</span>
    <!-- eslint-disable-next-line svelte/no-at-html-tags -->
    <code class="cmd shiki-inline">{@html highlight(cmd, 'bash')}</code>
    {#if running}
      <span class="running-dot" aria-label="running"></span>
    {:else if exitCode != null && exitCode !== 0}
      <span class="exit-badge" title="exit code">exit {exitCode}</span>
    {/if}
  </div>
  {#if out.length > 0}
    <pre class="out shiki-inline" class:failed={err != null}>{@html highlight(out, 'ansi')}</pre>
  {/if}
  {#if err}
    <div class="shell-error">{err}</div>
  {:else if cancelled}
    <div class="shell-cancelled">cancelled</div>
  {/if}
</div>

<style>
  .shell-step {
    display: flex;
    flex-direction: column;
    gap: 1px;
    padding: 4px 6px;
    scroll-margin-top: 32px;
  }
  .annotatable {
    position: relative;
  }
  .annotatable :global(.anno) {
    position: absolute;
    top: 2px;
    right: 4px;
    z-index: 2;
  }
  .ctx {
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--text-dim);
    line-height: 1.4;
  }
  .ctx-dir {
    color: var(--text-strong);
    font-weight: 600;
  }
  .line {
    display: flex;
    align-items: baseline;
    gap: 6px;
    min-width: 0;
  }
  .prompt {
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 12.5px;
    user-select: none;
    flex-shrink: 0;
  }
  .cmd {
    color: var(--text-strong);
    font-family: var(--font-mono);
    font-size: 12px;
    background: none;
    padding: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
    flex: 1;
  }
  /* Output sits flush under the prompt line — no border or pill
     surround so the row reads as one terminal capture, not a
     collapsed accordion. Cap at ~roughly 24 lines and let it scroll
     so a chatty command doesn't dominate the transcript. */
  .out {
    margin: 2px 0 0 18px;
    padding: 0;
    background: none;
    font-family: var(--font-mono);
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--text);
    white-space: pre-wrap;
    overflow-x: auto;
    max-height: 360px;
    overflow-y: auto;
  }
  .out.failed {
    color: var(--text-muted);
  }
  .exit-badge {
    flex-shrink: 0;
    font-family: var(--font-mono);
    font-size: 10.5px;
    font-weight: 600;
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 12%, transparent);
    border-radius: 4px;
    padding: 0 5px;
  }
  .shell-error {
    margin: 2px 0 0 18px;
    font-size: 11.5px;
    line-height: 1.45;
    color: var(--danger);
    white-space: pre-wrap;
  }
  .shell-cancelled {
    margin: 2px 0 0 18px;
    font-size: 11.5px;
    font-style: italic;
    color: var(--text-dim);
  }
  .running-dot {
    width: 6px;
    height: 6px;
    border-radius: 999px;
    background: var(--text-dim);
    animation: pulse 1.2s ease-in-out infinite;
    flex-shrink: 0;
  }
  @keyframes pulse {
    0%, 100% { opacity: 0.25; }
    50%      { opacity: 1; }
  }
</style>
