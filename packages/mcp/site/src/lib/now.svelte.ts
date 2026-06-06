// One shared clock in epoch seconds, advanced every second. Components read
// `now.value` so a running job's elapsed time updates live with a single timer
// for the whole page rather than one per card.
class Now {
  value = $state(Date.now() / 1000);
  #timer: ReturnType<typeof setInterval> | null = null;

  start(): void {
    if (this.#timer !== null) return;
    this.#timer = setInterval(() => {
      this.value = Date.now() / 1000;
    }, 1000);
  }

  stop(): void {
    if (this.#timer !== null) {
      clearInterval(this.#timer);
      this.#timer = null;
    }
  }
}

export const now = new Now();
