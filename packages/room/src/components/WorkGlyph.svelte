<script lang="ts">
  // Shared "agent process is running" indicator.
  //
  // - state="working" renders a braille spinner driven by the global
  //   spinnerFrame store, so every glyph in the app pulses in
  //   lockstep with the others.
  // - state="waiting" renders a static filled dot — used when the
  //   agent is paused on a permission request and needs a human.
  //
  // The component is presentation-only: callers pick the state from
  // domain data (thread.status, an in-flight turn flag, ...) and pass
  // it in.

  import { spinnerFrame } from '$lib/spinner';

  interface Props {
    mode: 'working' | 'waiting';
    /** Override the default size. Numbers are CSS px. */
    size?: number;
    /** Override the rendered label for screen readers. */
    label?: string;
  }

  let { mode, size, label }: Props = $props();

  let frame = $state('⠋');
  $effect(() => {
    if (mode !== 'working') return;
    return spinnerFrame.subscribe((v) => (frame = v));
  });

  let glyphStyle = $derived(size != null ? `font-size: ${size}px;` : '');
  let ariaLabel = $derived(label ?? (mode === 'working' ? 'Working' : 'Awaiting input'));
</script>

<span class="work-glyph {mode}" style={glyphStyle} aria-label={ariaLabel}>
  {#if mode === 'working'}{frame}{:else}●{/if}
</span>

<style>
  .work-glyph {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-family: var(--font-mono);
    font-variant-ligatures: none;
    pointer-events: none;
    user-select: none;
  }
  .work-glyph.working {
    color: var(--text-muted);
    font-size: 13px;
    /* The braille glyph itself does the "motion" via the cycling
       text. A tiny opacity breath underneath gives the indicator some
       low-frequency life so the eye keeps registering it as alive even
       when the glyph change is too small to notice peripherally. */
    animation: spinner-breath 2.4s ease-in-out infinite;
  }
  .work-glyph.waiting {
    color: var(--text-strong);
    font-size: 8px;
  }
  @keyframes spinner-breath {
    0%, 100% { opacity: 0.65; }
    50%      { opacity: 1; }
  }
  @media (prefers-reduced-motion: reduce) {
    .work-glyph.working {
      animation: none;
      opacity: 0.85;
    }
  }
</style>
