# Contract for agents editing this app

Edit `staging/` only. `src/` is the live tree Vite serves; the serve gate
typechecks `staging/` (`npm run check:staging`) and promotes it into `src/`
(`npm run promote`) when green. A direct `src/` edit ships unchecked code and
is overwritten by the next promote.

- Durable state lives in the store (`staging/lib/store.svelte.ts`): it
  survives hot reloads (`import.meta.hot` handoff) and full reloads
  (sessionStorage). Anything worth keeping across edits goes there, never in
  component-local state.
- Narrate: set `app.status` before each step so the page always says what
  you are doing right now.
- A section still being generated sets `loading: true` so skeletons render;
  clear the flag when its content lands.
- Keep updating until done: fill sections as results arrive, then set
  `app.done = true` with a final status when the work is complete.
