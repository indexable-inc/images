import type { Job, Resource } from './types';

// The live feed: polls the MCP server's REST API and exposes reactive state.
// Because the view binds to these arrays through keyed {#each} blocks, Svelte
// patches only what changed instead of rebuilding the DOM, so scroll position
// and open/closed source panels survive every refresh (the bug the old
// innerHTML-every-second page had).
class Feed {
  jobs = $state<Job[]>([]);
  resources = $state<Resource[]>([]);
  connected = $state(false);
  #timers: ReturnType<typeof setInterval>[] = [];

  async #pull<T>(path: string, apply: (value: T) => void): Promise<void> {
    try {
      const response = await fetch(path);
      if (!response.ok) throw new Error(String(response.status));
      apply((await response.json()) as T);
      this.connected = true;
    } catch {
      this.connected = false;
    }
  }

  start(intervalMs = 1000): void {
    if (this.#timers.length) return;
    const jobs = () => this.#pull<Job[]>('api/jobs', (v) => (this.jobs = v));
    const resources = () => this.#pull<Resource[]>('api/resources', (v) => (this.resources = v));
    void jobs();
    void resources();
    this.#timers.push(setInterval(jobs, intervalMs));
    this.#timers.push(setInterval(resources, intervalMs));
  }

  stop(): void {
    for (const timer of this.#timers) clearInterval(timer);
    this.#timers = [];
  }
}

export const feed = new Feed();
