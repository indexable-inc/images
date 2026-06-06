<script lang="ts">
  // Overlapping circular avatars showing who is currently viewing a
  // thread. Drives off the shared Loro presence doc. Self is excluded
  // from the visible stack by default.

  import type { PresenceEntry } from '$lib/loro';
  import { roomFor } from '$lib/store';
  import { loadIdentity } from '$lib/identity';
  import { nowTick, isIdle } from '$lib/activity';
  import { durationClock } from '$lib/time';
  import { sendViewer, receiveViewer } from '$lib/presenceAnim';
  import Avatar from '$components/Avatar.svelte';
  import ZzzIndicator from '$components/ZzzIndicator.svelte';

  interface Props {
    serverId: string;
    threadId: string;
    size?: number;
    max?: number;
    includeSelf?: boolean;
  }

  let { serverId, threadId, size = 20, max = 4, includeSelf = false }: Props = $props();
  let roomDoc = $derived(roomFor(serverId).doc);

  const self = loadIdentity();
  let presence = $state<PresenceEntry[]>([]);
  $effect(() => roomDoc.presenceList.subscribe((v) => (presence = v)));

  let now = $state(Date.now());
  const unsubNow = nowTick.subscribe((v) => (now = v));
  $effect(() => () => unsubNow());

  let viewers = $derived(
    presence.filter(
      (p) =>
        p.online &&
        p.viewing_thread_id === threadId &&
        now - p.last_seen_ms < 60000 &&
        (includeSelf || p.id !== self.id)
    )
  );
</script>

{#if viewers.length > 0}
  <span class="stack" style="--size: {size}px">
    {#each viewers.slice(0, max) as p (p.id)}
      {@const typing = p.typing_thread_id === threadId}
      {@const idle = !typing && isIdle(p.last_active_ms, now)}
      {@const idleFor = idle ? durationClock(now - p.last_active_ms) : ''}
      <span
        class="slot"
        class:typing
        class:idle
        data-tip={typing
          ? `${p.name} (typing)`
          : idle
            ? `${p.name} — idle ${idleFor}`
            : p.name}
        in:receiveViewer={{ key: p.id }}
        out:sendViewer={{ key: p.id }}
      >
        <span class="ring"></span>
        <Avatar name={p.name} github={p.github} size={size} />
        {#if idle}
          <span class="zzz-badge">
            <ZzzIndicator
              size={Math.max(7, Math.round(size * 0.55))}
              label={`${p.name} idle ${idleFor}`}
            />
          </span>
        {/if}
      </span>
    {/each}
    {#if viewers.length > max}
      <span class="more" data-tip={`${viewers.length} viewers`}>+{viewers.length - max}</span>
    {/if}
  </span>
{/if}

<style>
  .stack {
    display: inline-flex;
    align-items: center;
    flex-shrink: 0;
  }
  .slot {
    position: relative;
    display: inline-flex;
    margin-left: calc(var(--size) * -0.28);
    border-radius: 999px;
    box-shadow: 0 0 0 2px var(--bg);
    /* Soft breathing pulse so the avatar quietly says "alive". The
       transform on hover sits on top of this. */
    animation: breathe 3.6s ease-in-out infinite;
    transform-origin: center;
  }
  .slot:first-child {
    margin-left: 0;
  }
  .slot:hover {
    z-index: 2;
    animation-play-state: paused;
  }
  @keyframes breathe {
    0%, 100% { transform: scale(1); }
    50%      { transform: scale(1.045); }
  }

  /* The ring is an absolutely-positioned halo behind the avatar that
     grows + fades, giving each viewer a gentle "this person is here"
     pulse. When typing, the same ring shifts to the accent color and
     pulses faster + bigger. */
  .ring {
    position: absolute;
    inset: 0;
    border-radius: 999px;
    pointer-events: none;
    box-shadow: 0 0 0 0 color-mix(in srgb, var(--text-muted) 35%, transparent);
    animation: ring-pulse 3.6s ease-out infinite;
  }
  @keyframes ring-pulse {
    0%   { box-shadow: 0 0 0 0   color-mix(in srgb, var(--text-muted) 35%, transparent); }
    70%  { box-shadow: 0 0 0 6px color-mix(in srgb, var(--text-muted) 0%,  transparent); }
    100% { box-shadow: 0 0 0 0   color-mix(in srgb, var(--text-muted) 0%,  transparent); }
  }
  .slot.typing .ring {
    animation: ring-pulse-typing 1.1s ease-out infinite;
  }
  .slot.typing {
    box-shadow: 0 0 0 2px var(--bg), 0 0 0 3px var(--accent);
    animation: breathe-fast 1.1s ease-in-out infinite;
  }
  @keyframes breathe-fast {
    0%, 100% { transform: scale(1); }
    50%      { transform: scale(1.08); }
  }
  @keyframes ring-pulse-typing {
    0%   { box-shadow: 0 0 0 0   color-mix(in srgb, var(--accent) 55%, transparent); }
    70%  { box-shadow: 0 0 0 9px color-mix(in srgb, var(--accent) 0%,  transparent); }
    100% { box-shadow: 0 0 0 0   color-mix(in srgb, var(--accent) 0%,  transparent); }
  }

  /* Idle: stop the "alive" breathing + ring pulse and desaturate the
     avatar. The zzz badge does the talking from here. */
  .slot.idle {
    animation: none;
  }
  .slot.idle .ring {
    animation: none;
  }
  .slot.idle :global(img),
  .slot.idle :global(svg) {
    filter: grayscale(0.65);
    opacity: 0.75;
  }
  /* Anchor the zzz at the top-right of the avatar. position:absolute
     against .slot, which is the relative-positioned wrapper. */
  .zzz-badge {
    position: absolute;
    top: calc(var(--size) * -0.45);
    right: calc(var(--size) * -0.35);
    pointer-events: none;
    z-index: 3;
  }

  @media (prefers-reduced-motion: reduce) {
    .slot,
    .slot.typing,
    .ring {
      animation: none !important;
    }
  }
  /* Instant custom tooltip so peer names appear without the
     ~1s native-title delay. */
  .slot[data-tip]::after,
  .more[data-tip]::after {
    content: attr(data-tip);
    position: absolute;
    bottom: calc(100% + 6px);
    left: 50%;
    transform: translateX(-50%);
    padding: 3px 7px;
    border-radius: 5px;
    background: var(--text-strong);
    color: var(--bg-elev);
    font-size: 11px;
    font-family: var(--font-sans);
    font-weight: 500;
    white-space: nowrap;
    pointer-events: none;
    opacity: 0;
    transition: opacity 0.1s;
    z-index: 10;
  }
  .slot[data-tip]:hover::after,
  .more[data-tip]:hover::after {
    opacity: 1;
  }
  .more {
    position: relative;
    margin-left: 4px;
    color: var(--text-dim);
    font-size: 11px;
  }
</style>
