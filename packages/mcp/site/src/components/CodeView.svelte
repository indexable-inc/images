<script lang="ts">
  import type { Binding } from '$lib/types';

  // Highlighted source with live-value affordances. The server marks every
  // identifier token with `data-ix-name` (dashboard.py); here we join those
  // anchors with the run's `bindings` by name and underline the bound ones, then
  // surface the value on demand through one shared hover card with the value's
  // type, detail, and (for things with source) its definition site. The code HTML
  // is injected with {@html}, which Svelte does not scope or manage, so the
  // `.ix-bound` underline is styled globally (style.css) and the hover is driven
  // by event delegation on the host rather than per-node listeners. Inlay chips
  // are intentionally not rendered; `strip` still clears any stale ones.
  let { html, bindings = {} }: { html: string; bindings?: Record<string, Binding> } = $props();

  let host: HTMLElement;
  let card = $state<{ name: string; b: Binding; x: number; y: number } | null>(null);

  function strip(): void {
    if (!host) return;
    for (const chip of host.querySelectorAll('[data-ix-chip]')) chip.remove();
    for (const el of host.querySelectorAll('.ix-bound')) el.classList.remove('ix-bound');
  }

  function decorate(): void {
    if (!host) return;
    for (const el of host.querySelectorAll<HTMLElement>('[data-ix-name]')) {
      const name = el.dataset.ixName;
      const b = name ? bindings[name] : undefined;
      if (!name || !b) continue;
      el.classList.add('ix-bound');
      // Make the token keyboard-reachable so the card is not mouse-only.
      el.tabIndex = 0;
      // No inlay chip: the value shows only on hover/focus via the card below.
    }
  }

  // Re-decorate whenever the source or the values change. The injected content is
  // already in `host` when this effect runs (effects fire after DOM updates).
  $effect(() => {
    void html;
    void bindings;
    strip();
    decorate();
    return strip;
  });

  // Shared by mouse and keyboard: a FocusEvent and a MouseEvent both expose the
  // target/relatedTarget this reads, so one handler serves hover and focus.
  function enter(event: MouseEvent | FocusEvent): void {
    const target = (event.target as HTMLElement | null)?.closest<HTMLElement>('[data-ix-name]');
    const name = target?.dataset.ixName;
    const b = name ? bindings[name] : undefined;
    if (!target || !name || !b) return;
    const rect = target.getBoundingClientRect();
    card = { name, b, x: rect.left, y: rect.bottom + 5 };
  }

  function leave(event: MouseEvent | FocusEvent): void {
    const to = event.relatedTarget as HTMLElement | null;
    if (to?.closest?.('[data-ix-name]')) return;
    card = null;
  }
</script>

<!-- svelte-ignore a11y_mouse_events_have_key_events -->
<!-- The keyboard path uses focusin/focusout, which bubble from the {@html}-injected
     tokens; onfocus/onblur do not bubble, so they cannot model this delegated case. -->
<pre
  class="code ix-code"
  bind:this={host}
  onmouseover={enter}
  onmouseout={leave}
  onfocusin={enter}
  onfocusout={leave}
>{@html html}</pre>

{#if card}
  <div class="ix-card" style="left:{card.x}px; top:{card.y}px">
    <div class="ix-card-hd">
      <span class="ix-card-name">{card.name}</span>
      <span class="ix-card-type">{card.b.type}</span>
    </div>
    {#if card.b.detail}<pre class="ix-card-detail">{card.b.detail}</pre>{/if}
    {#if card.b.def}<div class="ix-card-def" title={card.b.def}>{card.b.def}</div>{/if}
  </div>
{/if}

<style>
  /* The source block. Matches JobCard's former pre.code: a quiet inset box where
     the colored tokens (inline-styled by the server) carry the meaning. */
  .ix-code {
    margin: 8px 0 0;
    padding: 9px 11px;
    max-height: 70vh;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-word;
    font-size: 12px;
    color: var(--dim);
    background: var(--inset);
    border: 1px solid var(--line);
  }

  /* The hover card is Svelte-managed, so its styles can stay scoped. Flat and
     square to match the rest of the dashboard; pinned to the viewport at the
     token's position (it dismisses on mouse-out, so it need not track scroll). */
  .ix-card {
    position: fixed;
    z-index: 40;
    max-width: 460px;
    padding: 7px 9px;
    background: var(--panel-2);
    border: 1px solid var(--line-2);
    font-size: 11.5px;
    pointer-events: none;
  }
  .ix-card-hd {
    display: flex;
    gap: 8px;
    align-items: baseline;
  }
  .ix-card-name {
    color: var(--text);
    font-weight: 600;
  }
  .ix-card-type {
    color: var(--muted);
    font-size: 10.5px;
  }
  .ix-card-detail {
    margin: 5px 0 0;
    max-height: 240px;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-word;
    color: var(--dim);
    font-size: 11.5px;
  }
  .ix-card-def {
    margin-top: 5px;
    padding-top: 5px;
    overflow: hidden;
    color: var(--accent);
    font-size: 10.5px;
    text-overflow: ellipsis;
    white-space: nowrap;
    border-top: 1px solid var(--line);
  }
</style>
