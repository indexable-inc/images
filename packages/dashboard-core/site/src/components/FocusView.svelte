<script lang="ts">
  import ExecBody from '$components/ExecBody.svelte';
  import { rendererFor } from '$lib/renderers';
  import { store, timeline } from '$lib/stream.svelte';
  import { ui, clearFocus, humanAge } from '$lib/ui.svelte';
  import type { Pane } from '$lib/types';

  // The single-resource view: one pane filling the stage, driven by the same
  // timeline as the board, so scrubbing and playback work here too. Opened from a
  // card's focus button; closed with Escape or the close button. A pane that is
  // not present at the scrubbed-to moment shows a placeholder rather than vanishing
  // the whole view.
  const key = $derived(ui.focusKey ?? '');
  const record = $derived(key ? store.panes[key] : undefined);
  const sep = String.fromCharCode(0x1f);
  const scope = $derived(key.includes(sep) ? key.slice(0, key.indexOf(sep)) : '');
  const pane = $derived(record ? ({ ...record, key, scope } as Pane) : undefined);

  const kind = $derived(pane?.kind ?? 'data');
  const isExec = $derived(kind === 'exec');
  const Body = $derived(rendererFor(kind, pane?.renderer));

  const refMs = $derived(
    timeline.source === 'live' && timeline.following ? Date.now() : timeline.position || timeline.maxTs,
  );
  const age = $derived(pane ? humanAge(pane.created_at, refMs) : '');
  // Escape (close) is handled centrally by the global keymap (lib/keys.svelte),
  // which clears the focus when an overlay is open.
</script>

<div class="focus">
  <div class="focus-head">
    <span class="focus-title">{pane?.title || '(pane)'}</span>
    {#if pane?.subtitle}<span class="focus-sub">{pane.subtitle}</span>{/if}
    <span class="focus-spacer"></span>
    {#if age}<span class="focus-age">created {age}</span>{/if}
    <span class="focus-kind">{kind}</span>
    <button class="focus-close" aria-label="close" onclick={clearFocus}>✕</button>
  </div>
  <div class="focus-body">
    {#if pane}
      <!-- Mirror PaneCard's `.pane > .body` wrapping so every renderer's CSS
           (scoped under `.pane`) applies here too. -->
      <div
        class="pane focus-pane"
        class:term={kind === 'terminal'}
        style={kind === 'terminal' ? 'font-size: 14px;' : ''}
      >
        <div class="body" class:term-body={kind === 'terminal'} class:html-body={kind === 'html'}>
          {#if isExec}
            <ExecBody {pane} expanded={true} />
          {:else}
            <Body {pane} />
          {/if}
        </div>
      </div>
    {:else}
      <div class="focus-absent">not present at this point in the timeline</div>
    {/if}
  </div>
</div>
