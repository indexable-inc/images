<script lang="ts">
  import { metrics } from '$lib/metrics.svelte';
  import { rendererFor } from '$lib/renderers';
  import { timeline } from '$lib/stream.svelte';
  import { ui, focusPane, humanAge } from '$lib/ui.svelte';
  import type { Pane } from '$lib/types';

  // One card on the canvas. PaneCard owns the chrome (the head and the card box)
  // and the sizing; the body is whatever renderer the pane's `kind` maps to, so
  // adding a kind never touches this file. The whole card is a drag handle (the
  // board gates text selection behind Alt), so the body must not start a drag.
  const FONT = 12;
  const CHROME_X = 12 * 2 + 1 * 2; // body padding-inline (12) + card border (1), both sides
  // Fixed card width for non-terminal panes. The board's pan/zoom scales it; a
  // terminal instead takes its exact natural cell width so its grid never wraps.
  const PANE_W = 360;

  let { pane, grabbed = false }: { pane: Pane; grabbed?: boolean } = $props();

  const kind = $derived(pane.kind ?? 'data');
  const isTerm = $derived(kind === 'terminal');
  const isExec = $derived(kind === 'exec');
  const alive = $derived(pane.alive !== false);
  const dead = $derived(isTerm && !alive);
  const cols = $derived(pane.cols && pane.cols > 0 ? pane.cols : 80);
  const title = $derived(pane.title || '(pane)');
  const subtitle = $derived(pane.subtitle ?? '');
  const showChip = $derived(!!pane.scope && pane.scope !== 'local');
  // A terminal sizes to its grid; everything else takes the fixed card width.
  const width = $derived(isTerm ? Math.ceil(cols * FONT * metrics.ratio) + CHROME_X : PANE_W);
  // Right-aligned meta: a terminal shows its geometry, other kinds show the kind
  // as a small badge so the canvas reads as heterogeneous at a glance.
  const meta = $derived(isTerm ? `${pane.rows ?? '?'}×${pane.cols ?? '?'}` : kind);
  const Body = $derived(rendererFor(kind, pane.renderer));

  // The LED reflects each kind's liveness: a terminal's alive flag, an exec's
  // running/ok status, otherwise just "present".
  const ledRun = $derived(isExec && pane.running === true);
  const ledErr = $derived((isExec && pane.ok === false) || dead);
  const ledLive = $derived(isTerm ? alive : isExec ? pane.ok === true : true);

  // The reference time for the age: wall-clock while following the live tail,
  // else the scrubbed-to moment, so a card shows its age at the replayed instant.
  const refMs = $derived(
    timeline.source === 'live' && timeline.following ? ui.clock : timeline.position || timeline.maxTs,
  );
  const age = $derived(humanAge(pane.created_at, refMs));

  function openFocus(e: PointerEvent): void {
    e.stopPropagation();
    focusPane(pane.key);
  }
</script>

<div
  class="pane"
  class:dead
  class:grabbed
  class:term={isTerm}
  class:focused={ui.focusKey === pane.key}
  style="width: {width}px;{isTerm ? ` font-size: ${FONT}px;` : ''}"
>
  <div class="head">
    <span
      class="led"
      class:live={ledLive}
      class:err={ledErr}
      class:run={ledRun}
      title={isTerm ? (alive ? 'running' : 'exited') : kind}
    ></span>
    <span class="cmd" title={title}>{title}</span>
    {#if subtitle}<span class="sub" title={subtitle}>{subtitle}</span>{/if}
    <span class="spacer"></span>
    {#if age}<span class="age" title={'created ' + age}>{age}</span>{/if}
    {#if showChip}<span class="chip" title={'producer ' + pane.scope}>{pane.scope}</span>{/if}
    <span class="size">{meta}</span>
    <button class="focus-btn" title="focus" aria-label="focus pane" onpointerdown={openFocus}>⤢</button>
  </div>
  <div class="body" class:term-body={isTerm} class:html-body={kind === 'html'}>
    <Body {pane} />
  </div>
</div>
