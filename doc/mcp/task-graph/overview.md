# task-graph

`packages/mcp/task-graph` is a standalone Vite + Svelte demo: an interactive
force-directed visualization of a ~100-node task dependency DAG in the Obsidian
graph-view style. It is the showcase for a tidy data-flow pattern, one schema and
one generator with two independent consumers: the Python
[`tasks`](../tool-providers/overview.md) module (a bundled provider) generates a
DAG into a real SQLite file, and the website reads that very file in the browser.
It is a demo, not part of the running server: it is run separately via npm and is
not built by [`default.nix`](../server/overview.md#build).

## The data: `tasks` (`packages/mcp/src/tasks/tasks/__init__.py`)

`tasks` generates and reads a deliberately tiny, acyclic dependency graph and is
the single source of truth for the demo's data (`tasks/__init__.py:1-7`). Public
surface:

- `generate(count, seed)` (`tasks/__init__.py:151`): a deterministic DAG where a
  task only depends on tasks defined before it, so edges never form a cycle.
- `status_of(task, by_id)` (`tasks/__init__.py:216`): derive `done`/`blocked`/
  `in-progress`/`ready` from whether a task's dependencies are complete, the same
  rule the website colors nodes by.
- `write`/`read` (`load`) (`tasks/__init__.py:244`, `tasks/__init__.py:270`):
  persist/load the DAG to/from SQLite.
- `seed(path, count, seed)` (`tasks/__init__.py:303`): generate and write in one
  call (default `tasks.sqlite`).
- `frame(source)` (`tasks/__init__.py:308`): a polars DataFrame with derived
  status, rendered as a styled table in the MCP dashboard.

The schema (`tasks/__init__.py:227`, `SCHEMA`) is two tables: `tasks`
(`id`, `title`, `category`, `estimate`, `complete`, `active`) and `deps`
(`task_id`, `depends_on`). It is pure stdlib at the core (`sqlite3`,
`dataclasses`); `polars` is imported lazily only for `frame`, so `import tasks`
works as a plain module outside the MCP kernel too.

Regenerate the committed asset with:

```sh
python3 -c 'import tasks; tasks.seed("public/tasks.sqlite")'
```

## The site (`packages/mcp/task-graph`)

The website reads `public/tasks.sqlite` in the browser via sql.js (a WASM build
of SQLite), so that one file is the contract between the two languages
(`src/lib/db.ts:1-3`). `loadTasks(url)` (`src/lib/db.ts:16`) fetches the asset and
runs `SELECT ... FROM deps` and `SELECT ... FROM tasks` against the in-browser
database, mirroring `tasks.SCHEMA`; the model is mirrored in `src/lib/types.ts`.

Layout:

| path | role |
| --- | --- |
| `index.html`, `src/main.ts`, `src/App.svelte` | the app shell and entry |
| `src/lib/db.ts` | load the DAG from `tasks.sqlite` via sql.js |
| `src/lib/types.ts` | the `Task`/`Category` model mirrored from `tasks.SCHEMA` |
| `src/lib/TaskGraph.svelte` | the `force-graph` + d3 collision-force visualization |
| `public/tasks.sqlite` | the committed data asset produced by `tasks.seed` |
| `vite.config.ts`, `svelte.config.js`, `tsconfig.json`, `package.json` | the Vite/Svelte build config |

Run it separately (`task-graph/README.md`): `npm install`, then `npm run dev`
(dev server), `npm run build` (type-check + production bundle), or
`npm run preview`. Interaction: color nodes by status or category, hover/select a
node to fade connected tasks by BFS hop-distance, toggle a layered DAG layout, and
search to jump to a node.

## Relation to the rest of the package

This component is independent of the running server: the `tasks` provider is one
of many [tool-providers](../tool-providers/overview.md) (it appears in `api()` and
the instructions like any other), and the site is a static front end over its
output. It does not touch the [kernel](../runtime/overview.md), the
[store](../sessions/overview.md), or the [dashboard hub](../dashboard/overview.md).
