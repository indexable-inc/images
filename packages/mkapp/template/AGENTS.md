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

## UI

The full shadcn-svelte component registry is vendored under
`staging/lib/components/ui/` (button, card, dialog, table, tabs, sidebar,
sonner, ...; only chart and form are omitted). Import through the `$lib`
alias, which resolves inside the tree being checked:

```svelte
import { Button } from '$lib/components/ui/button';
import * as Card from '$lib/components/ui/card';
```

Style with Tailwind (v4) utility classes. The theme maps the operator's
terminal palettes onto the shadcn variables (`bg-background`, `text-primary`,
`text-muted-foreground`, ...) and switches light/dark with the OS
automatically; use those semantic colors instead of hardcoded ones.
