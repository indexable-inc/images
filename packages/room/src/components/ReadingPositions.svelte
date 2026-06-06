<script lang="ts">
  // Right-gutter "who is reading where" overlay.
  //
  // For every other viewer of this thread, renders a small avatar
  // pinned to their last broadcast `scroll_pct` (0..1 fraction down
  // the transcript). The vertical position is interpolated with a
  // CSS transition so the bubble glides smoothly as the peer scrolls.
  // The whole overlay is absolutely positioned inside the transcript
  // scroller so it tracks scroll naturally as a sticky-feeling rail.

  import type { PresenceEntry } from '$lib/loro';
  import { roomFor } from '$lib/store';
  import { loadIdentity } from '$lib/identity';
  import { nowTick, isIdle } from '$lib/activity';
  import { durationClock } from '$lib/time';
  import Avatar from '$components/Avatar.svelte';
  import ZzzIndicator from '$components/ZzzIndicator.svelte';
  import { sendViewer, receiveViewer } from '$lib/presenceAnim';

  interface Props {
    serverId: string;
    threadId: string;
    /** Click on a reader bubble jumps our own scroll to roughly
     *  where that peer is. Caller passes scroll_pct (0..1). */
    onJumpTo?: (pct: number) => void;
  }

  let { serverId, threadId, onJumpTo }: Props = $props();
  let roomDoc = $derived(roomFor(serverId).doc);

  const self = loadIdentity();
  let presence = $state<PresenceEntry[]>([]);
  $effect(() => roomDoc.presenceList.subscribe((v) => (presence = v)));

  // Wall-clock tick so idle state flips on its own schedule (15s
  // since last input) regardless of when the next presence delta
  // happens to land.
  let now = $state(Date.now());
  const unsubNow = nowTick.subscribe((v) => (now = v));
  $effect(() => () => unsubNow());

  let readers = $derived(
    presence.filter(
      (p) =>
        p.id !== self.id &&
        p.online &&
        p.viewing_thread_id === threadId &&
        p.scroll_pct !== null &&
        now - p.last_seen_ms < 60000
    )
  );
</script>

{#if readers.length > 0}
  <div class="gutter" aria-hidden="true">
    {#each readers as p (p.id)}
      {@const scrollPct = p.scroll_pct ?? 0}
      {@const viewportPct = p.viewport_pct ?? 0.04}
      {@const idle = isIdle(p.last_active_ms, now)}
      {@const idleFor = idle ? durationClock(now - p.last_active_ms) : ''}
      <!-- scroll_pct is "fraction down the scroll *range*", which is
           why the band's top is scaled by (1 - viewport_pct) — at
           scrollTop=0 the viewport covers [0, viewport_pct]; at
           scrollTop=range it covers [1 - viewport_pct, 1]. -->
      {@const bandTopPct = scrollPct * (1 - viewportPct) * 100}
      {@const bandHeightPct = viewportPct * 100}
      <button
        type="button"
        class="reader"
        class:typing={p.typing_thread_id === threadId}
        class:idle
        style="top: {bandTopPct}%; height: {bandHeightPct}%"
        in:receiveViewer={{ key: p.id + ':scroll' }}
        out:sendViewer={{ key: p.id + ':scroll' }}
        data-tip={idle ? `${p.name} — idle ${idleFor}` : `Jump to ${p.name}`}
        onclick={() => onJumpTo?.(scrollPct)}
      >
        <span class="band"></span>
        <span class="avatar"><Avatar name={p.name} github={p.github} size={18} /></span>
        {#if idle}
          <span class="zzz-wrap">
            <ZzzIndicator size={11} label={`${p.name} idle ${idleFor}`} />
          </span>
        {/if}
      </button>
    {/each}
  </div>
{/if}

<style>
  .gutter {
    position: absolute;
    top: 8px;
    bottom: 14px;
    right: 6px;
    width: 38px;
    pointer-events: none;
    z-index: 4;
  }
  /* Each reader is now a full-height band representing what they
     can see, with the avatar pinned at the band's vertical centre.
     The band's `top` + `height` come from the inline style. */
  .reader {
    position: absolute;
    right: 0;
    width: 100%;
    pointer-events: auto;
    /* Button reset — we're using <button> for the a11y + native focus
     * behaviour, not the chrome. */
    background: none;
    border: 0;
    padding: 0;
    margin: 0;
    cursor: pointer;
    color: inherit;
    font: inherit;
    /* Linear easing matched to the upstream broadcast period (~80ms,
       12 Hz). With each step ending exactly when the next sample
       arrives, the bubble glides at the peer's actual scroll rate
       instead of accelerating + decelerating into every sample
       (which is what makes a cubic-bezier feel jolty under
       continuous scrolling). */
    transition:
      top 0.08s linear,
      height 0.08s linear;
  }
  .reader:hover .band {
    width: 5px;
    background: color-mix(in srgb, var(--text-muted) 70%, transparent);
  }
  .reader:hover .avatar {
    box-shadow:
      0 0 0 2px var(--bg-pane),
      0 0 0 3px var(--text-muted);
  }
  .reader:focus-visible {
    outline: none;
  }
  .reader:focus-visible .avatar {
    box-shadow:
      0 0 0 2px var(--bg-pane),
      0 0 0 3px var(--accent);
  }
  /* The band itself: a thin vertical bar the height of the viewer's
     visible viewport, rounded so short bands read as pills. */
  .band {
    position: absolute;
    top: 0;
    bottom: 0;
    right: 6px;
    width: 3px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--text-dim) 45%, transparent);
    transition: background 0.18s;
  }
  .reader.typing .band {
    background: var(--accent);
    width: 4px;
  }
  /* Avatar sits centred on the midpoint of the band, slightly to the
     left of it so the band stays visible alongside the head. */
  .avatar {
    position: absolute;
    top: 50%;
    right: 12px;
    transform: translateY(-50%);
    display: inline-flex;
    border-radius: 999px;
    box-shadow:
      0 0 0 2px var(--bg-pane),
      0 0 0 3px color-mix(in srgb, var(--text-dim) 40%, transparent);
    transition: box-shadow 0.18s;
  }
  .reader.typing .avatar {
    box-shadow:
      0 0 0 2px var(--bg-pane),
      0 0 0 3px var(--accent);
  }
  /* Idle peers: dim the band + desaturate the avatar so the active
     readers remain the most visually salient items in the gutter. */
  .reader.idle .band {
    background: color-mix(in srgb, var(--text-dim) 22%, transparent);
  }
  .reader.idle .avatar {
    filter: grayscale(0.55);
    opacity: 0.7;
  }
  /* The zzz floats just above-and-right of the avatar so it sits in
     empty gutter space (left of the avatar is the band; below is the
     next reader). */
  .zzz-wrap {
    position: absolute;
    top: calc(50% - 22px);
    right: 4px;
    pointer-events: none;
  }

  .reader[data-tip]::after {
    content: attr(data-tip);
    position: absolute;
    right: calc(100% + 8px);
    top: 50%;
    transform: translateY(-50%);
    padding: 3px 7px;
    border-radius: 5px;
    background: var(--text-strong);
    color: var(--bg-elev);
    font-size: 11px;
    font-family: var(--font-sans);
    font-weight: 500;
    white-space: nowrap;
    opacity: 0;
    transition: opacity 0.1s;
    pointer-events: none;
  }
  .reader:hover[data-tip]::after {
    opacity: 1;
  }

  @media (prefers-reduced-motion: reduce) {
    .reader,
    .band,
    .avatar {
      transition: none;
    }
  }
</style>
