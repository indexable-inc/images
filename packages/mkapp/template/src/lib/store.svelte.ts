/// <reference types="vite/client" />
// The durable app store. State written here survives:
//  - hot reloads: dispose() stashes a snapshot in import.meta.hot.data and
//    the next module instance rehydrates from it, so a promote never resets
//    the page;
//  - full reloads: a debounced sessionStorage mirror rehydrates on boot, the
//    safety net for the reloads HMR cannot cover (vite.config edits, manual
//    refresh).

export type Section = {
  id: string;
  title: string;
  /** True while the agent is still generating this section: skeletons show. */
  loading: boolean;
  body: string;
};

export type AppState = {
  /** One line: what the agent is doing right now. */
  status: string;
  /** Set once the whole task is finished. */
  done: boolean;
  sections: Section[];
};

const STORAGE_KEY = 'mkapp:state';
const PERSIST_DEBOUNCE_MS = 250;

function initialState(): AppState {
  return {
    status: 'waiting for the agent',
    done: false,
    sections: [
      {
        id: 'welcome',
        title: 'Welcome',
        loading: false,
        body:
          'Scaffolded by mkapp. The agent edits staging/ and this page hot ' +
          'reloads each promoted change without losing this store.',
      },
    ],
  };
}

function rehydrate(): AppState {
  const handoff = import.meta.hot?.data.appState as AppState | undefined;
  if (handoff) return handoff;
  try {
    const raw = sessionStorage.getItem(STORAGE_KEY);
    if (raw) return JSON.parse(raw) as AppState;
  } catch {
    // Unreadable storage means a fresh state, never a broken boot.
  }
  return initialState();
}

export const app = $state<AppState>(rehydrate());

if (import.meta.hot) {
  import.meta.hot.dispose((data) => {
    data.appState = $state.snapshot(app);
  });
}

// Debounced sessionStorage mirror. $effect.root gives module scope an effect
// context; JSON.stringify reads every property of the state proxy, so any
// deep change reschedules the write.
let persistTimer: ReturnType<typeof setTimeout> | undefined;
$effect.root(() => {
  $effect(() => {
    const snapshot = JSON.stringify(app);
    clearTimeout(persistTimer);
    persistTimer = setTimeout(() => {
      try {
        sessionStorage.setItem(STORAGE_KEY, snapshot);
      } catch {
        // Storage full or blocked: the HMR handoff still preserves state.
      }
    }, PERSIST_DEBOUNCE_MS);
  });
});
