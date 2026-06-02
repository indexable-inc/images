<script lang="ts">
  import {
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
  import { humanClock, humanElapsed } from '$lib/ui.svelte';

  // The replay bar: scrub the recorded history, play it back at speed, jump to
  // the live tail, switch to a saved recording, and copy a shareable deep link.
  // It drives the same document the board and focus view render, so all three
  // stay in lock-step.
  const SPEEDS = [1, 2, 4, 8];

  const hasHistory = $derived(timeline.changeCount > 1);
  const isLive = $derived(timeline.source === 'live');
  const value = $derived(timeline.following ? timeline.maxTs : timeline.position);
  const span = $derived(Math.max(0, timeline.maxTs - timeline.minTs));
  const elapsed = $derived(humanElapsed(value - timeline.minTs));
  const total = $derived(humanElapsed(span));

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
      // Clipboard blocked (insecure context): surface the URL so it can be
      // copied by hand.
      window.prompt('Share this view:', url);
    }
  }
</script>

<div class="timeline">
  <button class="tl-btn play" aria-label={timeline.playing ? 'pause' : 'play'} onclick={() => (timeline.playing ? pause() : play())} disabled={!hasHistory}>
    {timeline.playing ? '❚❚' : '▶'}
  </button>

  <button
    class="tl-btn live"
    class:on={timeline.following}
    onclick={goLive}
    title={isLive ? 'follow the live stream' : 'jump to the end'}
  >
    {isLive ? 'LIVE' : 'END'}
  </button>

  <input
    class="tl-range"
    type="range"
    min={timeline.minTs}
    max={timeline.maxTs}
    step="1"
    {value}
    oninput={onScrub}
    disabled={!hasHistory}
    aria-label="timeline position"
  />

  <span class="tl-time" title={humanClock(value)}>{elapsed} / {total}</span>

  <span class="tl-speed">
    {#each SPEEDS as s (s)}
      <button class="tl-btn" class:on={timeline.speed === s} onclick={() => setSpeed(s)}>{s}×</button>
    {/each}
  </span>

  {#if timeline.recordings.length}
    <select class="tl-rec" onchange={onPick} value={timeline.source}>
      <option value="live">● live</option>
      {#each timeline.recordings as rec (rec.id)}
        <option value={rec.id}>{humanClock(rec.started_ms)} · {(rec.bytes / 1024).toFixed(0)}kb</option>
      {/each}
    </select>
  {/if}

  <button class="tl-btn share" onclick={share} title="copy a link to this moment">
    {copied ? 'copied ✓' : 'share'}
  </button>
</div>
