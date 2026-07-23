import { onDestroy } from 'svelte';

/// Reactive wall-clock that ticks once a second. Shared via reference counting
/// so the interval only runs while at least one component is reading it.
/// Returns an object with a `value` getter; read `now.value` inside `$derived`
/// or markup to participate in reactivity.

const TICK_MS = 1000;

let current = $state(Date.now());
let interval: ReturnType<typeof setInterval> | null = null;
let refCount = 0;

function start(): void {
  if (interval !== null) return;
  current = Date.now();
  interval = setInterval(() => {
    current = Date.now();
  }, TICK_MS);
}

function stop(): void {
  if (interval === null) return;
  clearInterval(interval);
  interval = null;
}

export function useNow(): { readonly value: number } {
  refCount += 1;
  if (refCount === 1) start();
  onDestroy(() => {
    refCount -= 1;
    if (refCount === 0) stop();
  });
  return {
    get value(): number {
      return current;
    }
  };
}
