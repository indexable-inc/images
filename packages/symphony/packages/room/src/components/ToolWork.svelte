<script lang="ts">
  // Inline "work" rendering for a contiguous run of thinking, tool
  // calls, and tool results from one assistant turn.
  //
  // Design:
  //   - No outer "Show Work" toggle. Each step renders directly inline
  //     so the transcript reads as a single ordered narrative.
  //   - Thinking is plain italic muted prose with a thin left rule —
  //     no label, no header, no icon. It reads as the model's running
  //     commentary, not a separate UI element.
  //   - Tool calls render as compact one-line actions specific to the
  //     tool:
  //         Bash       →  $ <command>
  //         Read       →  Read <path>
  //         Edit/Write →  full diff (DiffBlock) when a patch is present
  //         Grep       →  Grep "<pattern>" in <path>
  //         Glob       →  Glob <pattern>
  //         TodoWrite  →  rendered checklist (no expansion needed)
  //         WebFetch   →  Fetch <url>
  //         WebSearch  →  Search "<query>"
  //         Task       →  Task <description>
  //         <other>    →  <tool_name>
  //     Clicking the row toggles the output / details body.
  //   - Tool output is decoded from the MCP envelope shape
  //     ({type:"text", content:"..."} or arrays of such) so the body
  //     reads as plain text rather than escaped JSON.

  import type { Message } from '$lib/types';
  import { parseDiffFiles } from '$lib/diff';
  import AnnotationFlag from './AnnotationFlag.svelte';
  import DiffBlock from './DiffBlock.svelte';
  import MarkdownBody from './MarkdownBody.svelte';
  import ShellWork from './ShellWork.svelte';
  import { highlightInline, highlighterReady } from '$lib/markdown';
  import {
    errorMessage,
    isError,
    isPlainObject,
    isRunning,
    strField,
    toolDisplay,
    toolStatus
  } from '$lib/toolWork';
  import IconChevronRight from '~icons/ph/caret-right';
  import IconTerminal from '~icons/ph/terminal-window';
  import IconFile from '~icons/ph/file-text';
  import IconPencil from '~icons/ph/pencil-simple';
  import IconSearch from '~icons/ph/magnifying-glass';
  import IconFolder from '~icons/ph/folder-open';
  import IconGlobe from '~icons/ph/globe';
  import IconWrench from '~icons/ph/wrench';
  import IconRobot from '~icons/ph/robot';
  import IconCircle from '~icons/ph/circle';
  import IconCircleDashed from '~icons/ph/circle-dashed';
  import IconCheckCircle from '~icons/ph/check-circle';

  interface Props {
    serverId: string;
    messages: Message[];
  }

  let { serverId, messages }: Props = $props();

  type TodoStatus = 'pending' | 'in_progress' | 'completed' | string;
  interface Todo {
    content: string;
    status: TodoStatus;
  }

  function asTodos(input: unknown): Todo[] | null {
    if (!isPlainObject(input)) return null;
    const arr = input.todos;
    if (!Array.isArray(arr)) return null;
    const out: Todo[] = [];
    for (const t of arr) {
      if (!isPlainObject(t)) continue;
      const content = typeof t.content === 'string' ? t.content : '';
      if (!content) continue;
      const status = typeof t.status === 'string' ? t.status : 'pending';
      out.push({ content, status });
    }
    return out;
  }

  function shortenPath(p: string): string {
    const norm = p.replace(/^\/+/, '');
    const parts = norm.split('/');
    if (parts.length <= 3) return p;
    return '…/' + parts.slice(-3).join('/');
  }

  function formatInputJSON(input: unknown): string {
    if (input == null) return '';
    if (typeof input === 'string') return input;
    try {
      return JSON.stringify(input, null, 2);
    } catch {
      return String(input);
    }
  }

  // Per-step open state. Most steps default closed (one-line action +
  // hidden output). Generic / unknown tools also default closed so the
  // transcript stays compact; click to drill in.
  let openMap = $state<Record<string, boolean>>({});
  function toggle(id: string) {
    openMap[id] = !openMap[id];
  }

  // Track shiki readiness so already-mounted Bash rows re-derive once
  // the highlighter loads and pick up syntax colors in place.
  let shikiReady = $state(false);
  const unsubShiki = highlighterReady.subscribe((v) => (shikiReady = v));
  $effect(() => () => unsubShiki());
  function highlightBash(cmd: string): string {
    void shikiReady; // re-derive when highlighter becomes available
    return highlightInline(cmd, 'bash');
  }
</script>

<div class="work" data-message-id={messages[0]?.id ?? ''}>
  {#each messages as m (m.id)}
    {#if m.kind === 'thinking'}
      <!-- Thinking: discrete, always visible, no chrome. -->
      <div class="thinking annotatable" data-message-id={m.id}>
        <MarkdownBody source={m.text ?? ''} />
        <AnnotationFlag {serverId} messageId={m.id} />
      </div>
    {:else if m.patch}
      <!-- Edit / Write / apply_patch: the diff is the action. -->
      <div class="diff-wrap annotatable" data-message-id={m.id}>
        {#each parseDiffFiles(m.patch) as file, i (i)}
          <DiffBlock {file} />
        {/each}
        <!-- A failed or declined patch apply still carries the diff;
             surface the failure so it can't read as a clean change. -->
        {@render errorBody(m)}
        <AnnotationFlag {serverId} messageId={m.id} />
      </div>
    {:else if m.tool_name === 'TodoWrite'}
      <!-- TodoWrite renders as a real checklist, no toggle needed. -->
      {@const todos = asTodos(m.tool_input)}
      {#if todos}
        <ul class="todos annotatable" data-message-id={m.id}>
          <AnnotationFlag {serverId} messageId={m.id} />
          {#each todos as t, i (i)}
            <li class="todo {t.status}">
              <span class="todo-mark">
                {#if t.status === 'completed'}
                  <IconCheckCircle width={14} height={14} />
                {:else if t.status === 'in_progress'}
                  <IconCircleDashed width={14} height={14} />
                {:else}
                  <IconCircle width={14} height={14} />
                {/if}
              </span>
              <span class="todo-text">{t.content}</span>
            </li>
          {/each}
        </ul>
      {/if}
    {:else if m.tool_name === 'Bash'}
      {@const cmd = strField(m.tool_input, 'command') ?? ''}
      {@const out = toolDisplay(m)}
      {@const hasOut = !isError(m) && out.length > 0}
      <div class="step annotatable" data-message-id={m.id}>
        <AnnotationFlag {serverId} messageId={m.id} />
        <button
          type="button"
          class="action bash"
          class:has-body={hasOut}
          class:running={isRunning(m)}
          onclick={() => hasOut && toggle(m.id)}
          aria-expanded={openMap[m.id] ?? false}
          disabled={!hasOut}
        >
          <span class="prompt">$</span>
          <!-- eslint-disable-next-line svelte/no-at-html-tags -->
          <code class="cmd shiki-inline">{@html highlightBash(cmd)}</code>
          {#if hasOut}
            <span class="chev" class:open={openMap[m.id]}>
              <IconChevronRight width={11} height={11} />
            </span>
          {:else if isRunning(m)}
            <span class="running-dot" aria-label="running"></span>
          {/if}
        </button>
        {@render outBody(m)}
      </div>
    {:else if m.tool_name === 'shell'}
      <!-- Codex shell tool. Lives in its own component because the
           layout (Starship-style two-line prompt, inline ANSI-
           highlighted output, no toggle) is meaningfully different
           from the single-line action rows everything else in this
           component uses. -->
      <ShellWork {serverId} message={m} />
    {:else if m.tool_name === 'Read' || m.tool_name === 'NotebookRead'}
      {@const path = strField(m.tool_input, 'file_path') ?? strField(m.tool_input, 'notebook_path') ?? ''}
      {@const out = toolDisplay(m)}
      {@const hasOut = !isError(m) && out.length > 0}
      <div class="step annotatable" data-message-id={m.id}>
        <AnnotationFlag {serverId} messageId={m.id} />
        <button
          type="button"
          class="action"
          class:has-body={hasOut}
          onclick={() => hasOut && toggle(m.id)}
          aria-expanded={openMap[m.id] ?? false}
          disabled={!hasOut}
        >
          <span class="ico"><IconFile width={13} height={13} /></span>
          <span class="verb">Read</span>
          <span class="arg" title={path}>{shortenPath(path)}</span>
          {#if hasOut}
            <span class="chev" class:open={openMap[m.id]}>
              <IconChevronRight width={11} height={11} />
            </span>
          {:else if isRunning(m)}
            <span class="running-dot" aria-label="running"></span>
          {/if}
        </button>
        {@render outBody(m)}
      </div>
    {:else if m.tool_name === 'Edit' || m.tool_name === 'MultiEdit' || m.tool_name === 'Write'}
      <!-- Edit/Write without a parsed patch: show as a generic action. -->
      {@const path = strField(m.tool_input, 'file_path') ?? ''}
      {@const out = toolDisplay(m)}
      {@const hasOut = !isError(m) && out.length > 0}
      <div class="step annotatable" data-message-id={m.id}>
        <AnnotationFlag {serverId} messageId={m.id} />
        <button
          type="button"
          class="action"
          class:has-body={hasOut}
          onclick={() => hasOut && toggle(m.id)}
          aria-expanded={openMap[m.id] ?? false}
          disabled={!hasOut}
        >
          <span class="ico"><IconPencil width={13} height={13} /></span>
          <span class="verb">{m.tool_name}</span>
          <span class="arg" title={path}>{shortenPath(path)}</span>
          {#if hasOut}
            <span class="chev" class:open={openMap[m.id]}>
              <IconChevronRight width={11} height={11} />
            </span>
          {:else if isRunning(m)}
            <span class="running-dot" aria-label="running"></span>
          {/if}
        </button>
        {@render outBody(m)}
      </div>
    {:else if m.tool_name === 'Grep'}
      {@const pattern = strField(m.tool_input, 'pattern') ?? ''}
      {@const path = strField(m.tool_input, 'path')}
      {@const out = toolDisplay(m)}
      {@const hasOut = !isError(m) && out.length > 0}
      <div class="step annotatable" data-message-id={m.id}>
        <AnnotationFlag {serverId} messageId={m.id} />
        <button
          type="button"
          class="action"
          class:has-body={hasOut}
          onclick={() => hasOut && toggle(m.id)}
          aria-expanded={openMap[m.id] ?? false}
          disabled={!hasOut}
        >
          <span class="ico"><IconSearch width={13} height={13} /></span>
          <span class="verb">Grep</span>
          <code class="arg quoted">"{pattern}"</code>
          {#if path}<span class="arg-dim">in {shortenPath(path)}</span>{/if}
          {#if hasOut}
            <span class="chev" class:open={openMap[m.id]}>
              <IconChevronRight width={11} height={11} />
            </span>
          {:else if isRunning(m)}
            <span class="running-dot" aria-label="running"></span>
          {/if}
        </button>
        {@render outBody(m)}
      </div>
    {:else if m.tool_name === 'Glob'}
      {@const pattern = strField(m.tool_input, 'pattern') ?? ''}
      {@const out = toolDisplay(m)}
      {@const hasOut = !isError(m) && out.length > 0}
      <div class="step annotatable" data-message-id={m.id}>
        <AnnotationFlag {serverId} messageId={m.id} />
        <button
          type="button"
          class="action"
          class:has-body={hasOut}
          onclick={() => hasOut && toggle(m.id)}
          aria-expanded={openMap[m.id] ?? false}
          disabled={!hasOut}
        >
          <span class="ico"><IconFolder width={13} height={13} /></span>
          <span class="verb">Glob</span>
          <code class="arg">{pattern}</code>
          {#if hasOut}
            <span class="chev" class:open={openMap[m.id]}>
              <IconChevronRight width={11} height={11} />
            </span>
          {:else if isRunning(m)}
            <span class="running-dot" aria-label="running"></span>
          {/if}
        </button>
        {@render outBody(m)}
      </div>
    {:else if m.tool_name === 'WebFetch'}
      {@const url = strField(m.tool_input, 'url') ?? ''}
      {@const out = toolDisplay(m)}
      {@const hasOut = !isError(m) && out.length > 0}
      <div class="step annotatable" data-message-id={m.id}>
        <AnnotationFlag {serverId} messageId={m.id} />
        <button
          type="button"
          class="action"
          class:has-body={hasOut}
          onclick={() => hasOut && toggle(m.id)}
          aria-expanded={openMap[m.id] ?? false}
          disabled={!hasOut}
        >
          <span class="ico"><IconGlobe width={13} height={13} /></span>
          <span class="verb">Fetch</span>
          <span class="arg" title={url}>{url}</span>
          {#if hasOut}
            <span class="chev" class:open={openMap[m.id]}>
              <IconChevronRight width={11} height={11} />
            </span>
          {:else if isRunning(m)}
            <span class="running-dot" aria-label="running"></span>
          {/if}
        </button>
        {@render outBody(m)}
      </div>
    {:else if m.tool_name === 'WebSearch'}
      {@const q = strField(m.tool_input, 'query') ?? ''}
      {@const out = toolDisplay(m)}
      {@const hasOut = !isError(m) && out.length > 0}
      <div class="step annotatable" data-message-id={m.id}>
        <AnnotationFlag {serverId} messageId={m.id} />
        <button
          type="button"
          class="action"
          class:has-body={hasOut}
          onclick={() => hasOut && toggle(m.id)}
          aria-expanded={openMap[m.id] ?? false}
          disabled={!hasOut}
        >
          <span class="ico"><IconSearch width={13} height={13} /></span>
          <span class="verb">Search</span>
          <span class="arg quoted">"{q}"</span>
          {#if hasOut}
            <span class="chev" class:open={openMap[m.id]}>
              <IconChevronRight width={11} height={11} />
            </span>
          {:else if isRunning(m)}
            <span class="running-dot" aria-label="running"></span>
          {/if}
        </button>
        {@render outBody(m)}
      </div>
    {:else if m.tool_name === 'Task' || m.tool_name === 'Agent'}
      {@const desc = strField(m.tool_input, 'description') ?? strField(m.tool_input, 'subagent_type') ?? ''}
      {@const out = toolDisplay(m)}
      {@const hasOut = !isError(m) && out.length > 0}
      <div class="step annotatable" data-message-id={m.id}>
        <AnnotationFlag {serverId} messageId={m.id} />
        <button
          type="button"
          class="action"
          class:has-body={hasOut}
          onclick={() => hasOut && toggle(m.id)}
          aria-expanded={openMap[m.id] ?? false}
          disabled={!hasOut}
        >
          <span class="ico"><IconRobot width={13} height={13} /></span>
          <span class="verb">Task</span>
          <span class="arg">{desc}</span>
          {#if hasOut}
            <span class="chev" class:open={openMap[m.id]}>
              <IconChevronRight width={11} height={11} />
            </span>
          {:else if isRunning(m)}
            <span class="running-dot" aria-label="running"></span>
          {/if}
        </button>
        {@render errorBody(m)}
        {#if hasOut && openMap[m.id]}
          <div class="task-output">
            <MarkdownBody source={out} />
          </div>
        {/if}
      </div>
    {:else}
      <!-- Generic tool fallback. -->
      {@const out = toolDisplay(m)}
      {@const hasOut = !isError(m) && (out.length > 0 || m.tool_input != null)}
      <div class="step annotatable" data-message-id={m.id}>
        <AnnotationFlag {serverId} messageId={m.id} />
        <button
          type="button"
          class="action"
          class:has-body={hasOut}
          onclick={() => hasOut && toggle(m.id)}
          aria-expanded={openMap[m.id] ?? false}
          disabled={!hasOut}
        >
          <span class="ico"><IconWrench width={13} height={13} /></span>
          <span class="verb">{m.tool_name ?? m.kind}</span>
          {#if hasOut}
            <span class="chev" class:open={openMap[m.id]}>
              <IconChevronRight width={11} height={11} />
            </span>
          {:else if isRunning(m)}
            <span class="running-dot" aria-label="running"></span>
          {/if}
        </button>
        {@render errorBody(m)}
        {#if hasOut && openMap[m.id]}
          {#if m.tool_input != null}
            <pre class="output dim">{formatInputJSON(m.tool_input)}</pre>
          {/if}
          {#if !isError(m) && out}
            <pre class="output">{out}</pre>
          {/if}
        {/if}
      </div>
    {/if}
  {/each}
</div>

<!-- Status-aware result body, shared by every tool renderer. An error
     is always visible (never hidden behind the collapse toggle) so a
     failed call can't read as a silent success; ok/empty output stays
     toggle-gated. -->
{#snippet errorBody(m: Message)}
  {@const err = errorMessage(m)}
  {@const detail = toolDisplay(m)}
  {#if err}
    <div class="result-error">
      <span class="result-error-label">error</span>
      <span class="result-error-msg">{err}</span>
      {#if detail}<pre class="result-error-detail">{detail}</pre>{/if}
    </div>
  {:else if toolStatus(m) === 'cancelled'}
    <div class="result-note">cancelled</div>
  {/if}
{/snippet}

{#snippet outBody(m: Message)}
  {@const out = toolDisplay(m)}
  {@render errorBody(m)}
  {#if !isError(m)}
    {#if openMap[m.id] && out}
      <pre class="output">{out}</pre>
    {:else if toolStatus(m) === 'empty'}
      <!-- Completed with no payload. A quiet, always-visible note so the
           row reads as done-with-nothing rather than the literal "null"
           it used to print or a dead, un-clickable row. -->
      <span class="empty-note">no output</span>
    {/if}
  {/if}
{/snippet}

<style>
  .work {
    margin: 8px 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    scroll-margin-top: 32px;
  }

  /* Thinking is a quiet voice-over. Italic muted prose with a thin
     left rule so it visually steps back from the rest of the turn
     without needing any label. */
  .thinking {
    margin: 6px 0;
    padding: 2px 0 2px 12px;
    border-left: 2px solid var(--border-hi);
    color: var(--text-muted);
    font-style: italic;
    scroll-margin-top: 32px;
  }
  .thinking :global(.md) {
    color: var(--text-muted);
    font-size: 12.5px;
    line-height: 1.6;
  }
  .thinking :global(.md p) {
    margin: 4px 0;
  }
  .thinking :global(.md code),
  .thinking :global(.md pre) {
    font-style: normal;
  }

  /* One step row + optional expanded body. */
  .step {
    display: flex;
    flex-direction: column;
    scroll-margin-top: 32px;
  }

  /* Anchor for the reviewer-note flag. The flag itself uses absolute
     positioning so it sits flush with the top-right of each row
     without forcing every branch to wrap its action button in a flex
     container — see AnnotationFlag.svelte for the popover layout. */
  .annotatable {
    position: relative;
  }
  .annotatable :global(.anno) {
    position: absolute;
    top: 2px;
    right: 2px;
    z-index: 2;
  }
  .diff-wrap.annotatable :global(.anno) {
    top: 6px;
    right: 8px;
  }
  .todos.annotatable :global(.anno) {
    top: 4px;
    right: 4px;
  }
  .action {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    align-self: flex-start;
    max-width: 100%;
    padding: 3px 8px 3px 6px;
    border-radius: 6px;
    color: var(--text-muted);
    font-size: 12.5px;
    text-align: left;
    transition: background 0.1s, color 0.1s;
  }
  .action.has-body {
    cursor: pointer;
  }
  .action.has-body:hover {
    background: var(--bg-hover);
    color: var(--text);
  }
  .action[disabled] {
    cursor: default;
  }

  .ico {
    display: inline-flex;
    color: var(--text-dim);
    flex-shrink: 0;
  }
  .verb {
    color: var(--text);
    font-weight: 500;
  }
  .arg {
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 11.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .arg.quoted {
    color: var(--text);
  }
  .arg-dim {
    color: var(--text-dim);
    font-size: 11.5px;
  }
  .chev {
    display: inline-flex;
    color: var(--text-dim);
    transition: transform 0.15s ease;
    transform-origin: center;
    margin-left: 2px;
  }
  .chev.open {
    transform: rotate(90deg);
  }

  /* Bash: terminal prompt look. */
  .action.bash .prompt {
    color: var(--text-dim);
    font-family: var(--font-mono);
    font-size: 12.5px;
    user-select: none;
  }
  .action.bash .cmd {
    color: var(--text-strong);
    font-family: var(--font-mono);
    font-size: 12px;
    background: none;
    padding: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .action.bash.running .cmd {
    color: var(--text);
  }

  .running-dot {
    width: 6px;
    height: 6px;
    border-radius: 999px;
    background: var(--text-dim);
    animation: pulse 1.2s ease-in-out infinite;
  }
  @keyframes pulse {
    0%, 100% { opacity: 0.25; }
    50%      { opacity: 1; }
  }

  .output {
    margin: 4px 0 6px 22px;
    padding: 8px 10px;
    background: var(--bg-pill);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--text);
    white-space: pre-wrap;
    overflow-x: auto;
    max-height: 360px;
    overflow-y: auto;
  }
  .output.dim {
    color: var(--text-muted);
  }

  /* A failed tool call. Always visible, danger-tinted, with the failure
     reason on its own line and any captured output below it. */
  .result-error {
    margin: 4px 0 6px 22px;
    padding: 7px 10px;
    background: color-mix(in srgb, var(--danger) 8%, var(--bg-pill));
    border: 1px solid color-mix(in srgb, var(--danger) 40%, var(--border));
    border-radius: 6px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .result-error-label {
    align-self: flex-start;
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--danger);
  }
  .result-error-msg {
    color: var(--text);
    font-size: 12px;
    line-height: 1.45;
    white-space: pre-wrap;
  }
  .result-error-detail {
    margin: 2px 0 0;
    padding: 6px 8px;
    background: var(--bg-pill);
    border: 1px solid var(--border);
    border-radius: 5px;
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 1.5;
    color: var(--text-muted);
    white-space: pre-wrap;
    overflow-x: auto;
    max-height: 280px;
    overflow-y: auto;
  }
  .result-note {
    margin: 2px 0 4px 22px;
    font-size: 11.5px;
    font-style: italic;
    color: var(--text-dim);
  }
  .empty-note {
    display: inline-block;
    margin: 1px 0 2px 22px;
    font-size: 11px;
    font-style: italic;
    color: var(--text-dim);
  }
  .task-output {
    margin: 4px 0 6px 22px;
    padding: 8px 12px;
    background: var(--bg-pill);
    border: 1px solid var(--border);
    border-radius: 6px;
    max-height: 420px;
    overflow-y: auto;
  }

  /* TodoWrite checklist. */
  .todos {
    list-style: none;
    margin: 6px 0;
    padding: 8px 12px 8px 10px;
    border-left: 2px solid var(--border-hi);
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .todo {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    font-size: 12.5px;
    color: var(--text);
    line-height: 1.45;
  }
  .todo-mark {
    display: inline-flex;
    flex-shrink: 0;
    margin-top: 2px;
    color: var(--text-dim);
  }
  .todo.in_progress .todo-mark {
    color: var(--text);
  }
  .todo.completed .todo-mark {
    color: var(--text-muted);
  }
  .todo.completed .todo-text {
    color: var(--text-muted);
    text-decoration: line-through;
    text-decoration-color: var(--text-dim);
  }
  .todo.in_progress .todo-text {
    color: var(--text-strong);
    font-weight: 500;
  }

  .diff-wrap {
    scroll-margin-top: 32px;
  }
</style>
