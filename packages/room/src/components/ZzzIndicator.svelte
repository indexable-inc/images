<script lang="ts">
  // Tiny stacked-"zzz" animation rendered next to an idle peer's
  // avatar. Three z glyphs of increasing size drift up and to the
  // right with staggered delays, giving the classic cartoon-sleep
  // feel. Sized off a single `size` prop so callers can tune it to
  // the surrounding avatar.

  interface Props {
    /** Base glyph size in px. The biggest z is roughly this size. */
    size?: number;
    /** Tooltip / aria-label text. */
    label?: string;
  }

  let { size = 12, label = 'Idle' }: Props = $props();
</script>

<span
  class="zzz"
  style="--zsize: {size}px"
  role="img"
  aria-label={label}
  title={label}
>
  <span class="z z1">z</span>
  <span class="z z2">z</span>
  <span class="z z3">z</span>
</span>

<style>
  .zzz {
    position: relative;
    display: inline-block;
    /* Reserve a roughly square box so the floating zs don't push
       surrounding flex content around as they animate. */
    width: calc(var(--zsize) * 1.6);
    height: calc(var(--zsize) * 1.6);
    vertical-align: middle;
    color: color-mix(in srgb, var(--text-muted) 75%, transparent);
    font-family: var(--font-sans);
    font-style: italic;
    font-weight: 700;
    line-height: 1;
    pointer-events: none;
    user-select: none;
  }

  .z {
    position: absolute;
    /* All three glyphs start stacked at the bottom-left of the box
       and float toward the top-right. */
    left: 0;
    bottom: 0;
    opacity: 0;
    animation: zzz-float 2.4s ease-in-out infinite;
    will-change: transform, opacity;
  }

  .z1 {
    font-size: calc(var(--zsize) * 0.62);
    animation-delay: 0s;
  }
  .z2 {
    font-size: calc(var(--zsize) * 0.82);
    animation-delay: 0.8s;
  }
  .z3 {
    font-size: var(--zsize);
    animation-delay: 1.6s;
  }

  @keyframes zzz-float {
    0% {
      transform: translate(0, 0) rotate(0deg) scale(0.7);
      opacity: 0;
    }
    15% {
      opacity: 1;
    }
    80% {
      opacity: 1;
    }
    100% {
      transform: translate(calc(var(--zsize) * 0.9), calc(var(--zsize) * -1.2))
        rotate(-12deg) scale(1);
      opacity: 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .z {
      animation: none;
    }
    /* Without motion, fall back to a single static glyph so the
       indicator still communicates "idle" at a glance. */
    .z1,
    .z2 {
      display: none;
    }
    .z3 {
      opacity: 0.85;
    }
  }
</style>
