import type { Job, Resource, Cell } from './types';

// The live feed: polls the MCP server's REST API and exposes reactive state.
// The view binds to these arrays through keyed {#each} blocks, so Svelte patches
// only what changed instead of rebuilding the DOM, and scroll position and open
// panels survive every refresh.
//
// Each poll only reassigns an array when the payload actually changed (compared
// by serialized form). An idle session therefore produces no reactive churn, so
// nothing re-renders or fights the user's scroll between real updates.
class Feed {
  jobs = $state<Job[]>([]);
  resources = $state<Resource[]>([]);
  cells = $state<Cell[]>([]);
  connected = $state(false);
  #timers: ReturnType<typeof setInterval>[] = [];
  #raw = new Map<string, string>();

  async #pull<T>(path: string, key: string, apply: (value: T) => void): Promise<void> {
    try {
      const response = await fetch(path);
      if (!response.ok) throw new Error(String(response.status));
      const body = await response.text();
      this.connected = true;
      // Skip the reactive write when nothing changed since the last poll, so an
      // idle feed never triggers a re-render (or a scroll nudge).
      if (this.#raw.get(key) === body) return;
      this.#raw.set(key, body);
      apply(JSON.parse(body) as T);
    } catch {
      this.connected = false;
    }
  }

  start(intervalMs = 1000): void {
    if (this.#timers.length) return;
    const jobs = () => this.#pull<Job[]>('api/jobs', 'jobs', (v) => (this.jobs = v));
    const resources = () =>
      this.#pull<Resource[]>('api/resources', 'resources', (v) => (this.resources = v));
    const cells = () => this.#pull<Cell[]>('api/cells', 'cells', (v) => (this.cells = v));
    const tick = () => {
      void jobs();
      void resources();
      void cells();
    };
    tick();
    this.#timers.push(setInterval(tick, intervalMs));
  }

  stop(): void {
    for (const timer of this.#timers) clearInterval(timer);
    this.#timers = [];
  }
}

export const feed = new Feed();
