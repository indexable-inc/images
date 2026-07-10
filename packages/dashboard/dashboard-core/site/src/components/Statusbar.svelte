<script lang="ts">
  // The bottom status bar (24px): a live dot + session/run counts on the left,
  // key hints in the center, and an always-visible timeline scrubber on the right.
  // The scrubber shows the current position and a LIVE tag; clicking the controls
  // button opens a compact popover with play/pause, speed, share, and the
  // recording picker (the controls that used to live in the hover-hidden pill).
  import {
    store,
    timeline,
    play,
    pause,
    scrubTo,
    goLive,
    setSpeed,
    loadRecording,
    leaveRecording,
    shareUrl,
  } from '$lib/stream.svelte';
  import { humanClock, recordingLabel } from '$lib/ui.svelte';

  let { sessionCount, runCount }: { sessionCount: number; runCount: number } = $props();

  const SPEEDS = [1, 2, 4, 8];
  const hasHistory = $derived(timeline.changeCount > 1);
  const isLive = $derived(timeline.source === 'live');
  const value = $derived(timeline.following ? timeline.maxTs : timeline.position);
  const following = $derived(timeline.following);

  let controlsOpen = $state(false);
  let copied = $state(false);

  function onScrub(e: Event): void {
    scrubTo(Number((e.target as HTMLInputElement).value));
  }

  function onPick(e: Event): void {
    const id = (e.target as HTMLSelectElement).value;
    if (id === 'live') leaveRecording();
    else void loadRecording(id);
  }

  async function share(): Promise<void> {
    const url = shareUrl();
    try {
      await navigator.clipboard.writeText(url);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      window.prompt('Share this view:', url);
    }
  }
</script>

<footer class="statusbar">
  <span class="status-live" class:on={store.live}>
    <span class="dot"></span>{store.live ? 'live' : store.status}
  </span>
  <span class="status-sep">|</span>
  <span class="status-counts">{sessionCount} {sessionCount === 1 ? 'session' : 'sessions'} · {runCount} {runCount === 1 ? 'run' : 'runs'}</span>

  <span class="hints">
    <kbd>j/k</kbd>move <kbd>o</kbd>open <kbd>/</kbd>filter <kbd>?</kbd>help
  </span>

  <div class="scrubber-wrap">
    <button
      class="scrub-tag"
      class:live={following && isLive}
      class:seeking={timeline.seeking}
      onclick={() => (controlsOpen = !controlsOpen)}
      title={timeline.seeking ? 'loading frame…' : 'timeline controls'}
    >{following ? (isLive ? 'LIVE' : 'END') : humanClock(value)}</button>
    <input
      class="scrubber"
      type="range"
      min={timeline.minTs}
      max={timeline.maxTs}
      step="1"
      {value}
      oninput={onScrub}
      disabled={!hasHistory}
      aria-label="timeline position"
      title={humanClock(value)}
    />

    {#if controlsOpen}
      <div class="scrub-pop">
        <button class="tl-btn" aria-label={timeline.playing ? 'pause' : 'play'} onclick={() => (timeline.playing ? pause() : play())} disabled={!hasHistory}>
          {timeline.playing ? '❚❚' : '▶'}
        </button>
        <button class="tl-btn live" class:on={following} onclick={goLive} title={isLive ? 'follow the live stream' : 'jump to the end'}>
          {isLive ? 'LIVE' : 'END'}
        </button>
        <span class="tl-speed">
          {#each SPEEDS as s (s)}
            <button class="tl-btn" class:on={timeline.speed === s} onclick={() => setSpeed(s)}>{s}×</button>
          {/each}
        </span>
        {#if timeline.recordings.length}
          <select class="tl-rec" onchange={onPick} value={timeline.source} title="replay a saved session">
            <option value="live">● live</option>
            {#each timeline.recordings as rec (rec.id)}
              <option value={rec.id}>{recordingLabel(rec.started_ms, rec.updated_ms, Date.now())}</option>
            {/each}
          </select>
        {/if}
        <button class="tl-btn" onclick={share} title="copy a link to this moment">
          {copied ? 'copied ✓' : 'share'}
        </button>
      </div>
    {/if}
  </div>
</footer>

<style>
  .statusbar {
    flex: none;
    height: 24px;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 0 12px;
    background: var(--panel);
    border-top: 1px solid var(--edge);
    font-family: var(--mono);
    font-size: 11px;
    color: var(--ink-dim);
  }
  .status-live {
    display: flex;
    align-items: center;
    gap: 5px;
    color: var(--ink-faint);
  }
  .status-live.on {
    color: var(--live);
  }
  .status-live .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--ink-faint);
  }
  .status-live.on .dot {
    background: var(--live);
    animation: led-pulse 1.4s ease-in-out infinite;
  }
  @media (prefers-reduced-motion: reduce) {
    .status-live.on .dot {
      animation: none;
    }
  }
  .status-sep {
    color: var(--edge-strong);
  }
  .status-counts {
    color: var(--ink-dim);
    font-variant-numeric: tabular-nums;
  }
  .hints {
    color: var(--ink-faint);
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .hints kbd {
    font-family: var(--mono);
    background: var(--elev, var(--panel));
    border: 1px solid var(--edge);
    padding: 0 4px;
    color: var(--ink-dim);
    font-size: 10px;
  }

  .scrubber-wrap {
    margin-left: auto;
    position: relative;
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
  }
  .scrub-tag {
    font: inherit;
    font-size: 10px;
    letter-spacing: 0.08em;
    color: var(--ink-dim);
    background: none;
    border: 0;
    padding: 0;
    cursor: pointer;
    min-width: 44px;
    text-align: right;
  }
  .scrub-tag.live {
    color: var(--live);
  }
  .scrub-tag.seeking {
    color: var(--accent);
    animation: seek-pulse 1s ease-in-out infinite;
  }
  @keyframes seek-pulse {
    50% {
      opacity: 0.45;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .scrub-tag.seeking {
      animation: none;
    }
  }
  .scrub-tag:hover {
    color: var(--ink);
  }
  .scrubber {
    width: 200px;
    accent-color: var(--accent);
    cursor: pointer;
  }
  .scrubber:disabled {
    opacity: 0.4;
    cursor: default;
  }

  /* The controls popover, anchored above the scrubber. */
  .scrub-pop {
    position: absolute;
    right: 0;
    bottom: calc(100% + 8px);
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
    background: var(--elev, var(--panel));
    border: 1px solid var(--edge);
    box-shadow: 0 8px 28px -10px rgba(0, 0, 0, 0.5);
  }
  .tl-btn {
    font: inherit;
    color: var(--ink-dim);
    background: var(--panel);
    border: 1px solid var(--edge);
    padding: 3px 8px;
    cursor: pointer;
    min-width: 26px;
  }
  .tl-btn:hover:not(:disabled) {
    color: var(--ink);
    border-color: var(--edge-strong);
  }
  .tl-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .tl-btn.on {
    color: var(--bg);
    background: var(--accent);
    border-color: var(--accent);
  }
  .tl-btn.live.on {
    background: var(--live);
    border-color: var(--live);
  }
  .tl-speed {
    display: flex;
    gap: 3px;
  }
  .tl-rec {
    font: inherit;
    color: var(--ink-dim);
    background: var(--panel);
    border: 1px solid var(--edge);
    padding: 3px 6px;
    cursor: pointer;
    max-width: 160px;
  }
</style>
