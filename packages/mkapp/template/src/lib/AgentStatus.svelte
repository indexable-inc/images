<script lang="ts">
  import Skeleton from './components/ui/Skeleton.svelte';
  import { app } from './store.svelte';

  const loading = $derived(app.sections.filter((section) => section.loading));
</script>

<div class="strip" role="status">
  <span class="led" class:busy={!app.done}></span>
  <span class="text">{app.status}</span>
  {#if loading.length > 0}
    <span class="pending">{loading.length} loading</span>
  {/if}
</div>

{#if loading.length > 0}
  <div class="placeholders">
    {#each loading as section (section.id)}
      <div class="placeholder">
        <span class="label">{section.title}</span>
        <Skeleton width="62%" height="10px" />
        <Skeleton width="84%" height="10px" />
      </div>
    {/each}
  </div>
{/if}

<style>
  .strip {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
    background: var(--panel);
    border: 1px solid var(--edge);
    border-radius: var(--radius);
    font-family: var(--mono);
    font-size: 12px;
  }
  .led {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    border: 1px solid var(--live);
    background: transparent;
  }
  .led.busy {
    border-color: transparent;
    background: var(--accent);
    animation: led-pulse 1.4s ease-in-out infinite;
  }
  @keyframes led-pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.35;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .led.busy {
      animation: none;
    }
  }
  .text {
    color: var(--ink);
  }
  .pending {
    margin-left: auto;
    color: var(--ink-faint);
  }
  .placeholders {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-top: 10px;
  }
  .placeholder {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 12px 14px;
    background: var(--panel);
    border: 1px dashed var(--edge-strong);
    border-radius: var(--radius);
  }
  .label {
    color: var(--ink-faint);
    font-size: 12px;
  }
</style>
