<script lang="ts">
  type Props = {
    width?: string;
    height?: string;
  };

  let { width = '100%', height = '14px' }: Props = $props();
</script>

<div class="skeleton" style:width style:height></div>

<style>
  /* The shadcn skeleton: a muted rounded block with a slow pulse. */
  .skeleton {
    border-radius: calc(var(--radius) - 4px);
    background: var(--edge-strong);
    animation: pulse 2s cubic-bezier(0.4, 0, 0.6, 1) infinite;
  }
  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.45;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .skeleton {
      animation: none;
    }
  }
</style>
