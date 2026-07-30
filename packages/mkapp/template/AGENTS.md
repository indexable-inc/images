# Contract for agents editing this app

Edit `staging/` only. `src/` is the live tree Vite serves; the serve gate
typechecks and tests `staging/` (`npm run check:staging`) and promotes it into
`src/` (`npm run promote`) when green. A direct `src/` edit ships unchecked code
and is overwritten by the next promote.

## Change the page through the update surface

`staging/lib/live.ts` is where you write. It re-runs on every promote, against
the store a reader already has loaded, so a statement there lands on their page
in a few seconds without a refresh. Editing the seed in `plan.ts` does nothing
for a page that is already open: it rehydrated its store on load and will never
read the seed again.

```ts
import * as page from './store.svelte.ts';

page.by('vrack');            // attribute what follows; relayed findings keep their author
page.narrate('measuring the CVE gate');
page.add({ id: 'vrack-acl', title: '...', loading: false, body: '...' }, 'top');
page.say('vrack-acl', 'Reproduced on a second host; PR 9102.');
page.set('vrack-acl', { loading: true });
page.remove('vrack-acl');
```

- `add` is **insert-if-absent**, not upsert. To change a section that exists,
  use `set`. (An upsert would re-apply your literal on every promote and revert
  whatever `say` had appended since.)
- `say` appends once; re-running it adds nothing.
- **Declare each field once per file.** Two `narrate` calls with different
  values genuinely toggle the status on every promote, and the history records
  both transitions each time, because the statements are applied eagerly.
- Deleting a statement does **not** undo it. The change is already in the
  reader's store. Push the inverse, or call `page.reset()` once and remove it.
- Set `loading: true` on a section still being written so skeletons render, and
  finish with `page.narrate('...', true)`.

## Version history

Every mutation records itself. The reader presses `H` for the panel: who
changed what, when, newest first, and clicking a row shows the page exactly as
it stood just after that change.

The log records **state transitions, not calls**, so re-running an unchanged
`live.ts` appends nothing. That property is what makes it safe to leave
statements in the file forever, and `plan.test.ts` pins it.

What a reader can recover: every change since **their own** first load, with
author, time, the mutation's intent, the section it touched, and the page as of
any point. What they cannot: anything from before they opened the page (the log
lives in their session, it is not served), detail trimmed once the log passes
its cap, and the content of a change made outside the update surface — only
that one happened, flagged in red.

## Where the code lives

- `lib/plan.ts` — the document model and what each mutation changes. Pure.
- `lib/history.ts` — the log: change ops, inversion, reconstruction. Pure.
- `lib/shape.ts` — reads a stale storage mirror back without blanking the page.
- `lib/store.svelte.ts` — the reactive shell: runes, persistence, the public API.
- `lib/*.test.ts` — run by the gate with `node --test`. Add to them.

Relative imports inside `lib/` carry the `.ts` extension, because `node --test`
strips types without rewriting specifiers.

**Changing the store's shape is safe, but only through `shape.ts`.** A stale
mirror is reconciled field by field: what is valid is kept, what is missing is
filled from the seed, what is unrecognised is dropped. Add a field to `AppState`
and give it a default there, and a reader who has had the page open all
afternoon keeps their content and gains the field on the same promote. Skipping
that step is ENG-11106: an unchecked cast, components indexing into `undefined`,
and a blank page behind a green gate.

## UI

The full shadcn-svelte component registry is vendored under
`staging/lib/components/ui/` (button, card, dialog, table, tabs, sidebar,
sonner, ...; only chart and form are omitted). Import through the `$lib` alias,
which resolves inside the tree being checked:

```svelte
import { Button } from '$lib/components/ui/button';
import * as Card from '$lib/components/ui/card';
```

Style with Tailwind (v4) utility classes. The theme maps the operator's
terminal palettes onto the shadcn variables (`bg-background`, `text-primary`,
`text-muted-foreground`, ...) and switches light/dark with the OS
automatically; use those semantic colors instead of hardcoded ones.
