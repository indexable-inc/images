"""Composable guidance fragments for the MCP surface.

The server instructions AND every tool description are assembled from the named
fragments below rather than spelled out in full at each site, so a rule -- how a
blocking call wedges the kernel, what a cell must return, how to page a job -- is
authored once here and reused wherever it applies. `compose(*parts)` joins
fragments into one normalized block; `tools.py` wires them together (and derives
the per-tool overview from the registry, so nothing restates a tool by hand).
"""

from __future__ import annotations

from . import registry


def compose(*parts: str) -> str:
    """Join guidance fragments into a single space-separated block, collapsing each
    fragment's incidental whitespace so callers can pick any subset in any order."""
    return " ".join(" ".join(part.split()) for part in parts if part)


def modules_index() -> str:
    """The bundled-module index for the instructions, generated from `registry`
    so a module is named in exactly one place and NO signature is ever copied into
    prose (the exact signatures come from `api()` / `help()`). Adding a module to
    the registry lists it here for free."""
    preimported = ", ".join(f"`{name}`" for name in registry.preimport_names())
    mods = "; ".join(f"`{m.name}` \u2014 {m.tagline}" for m in registry.MODULES)
    libs = ", ".join(
        f"`{lib.name}`" + (f" ({lib.note})" if lib.note else "") for lib in registry.LIBRARIES
    )
    return (
        f"Bundled tooling, no install step: every module is pre-bound in the namespace, so use "
        f"any of them with no import (e.g. `await maps.nearby(...)` works directly). {preimported} "
        "load eagerly; the rest are bound lazily and import themselves on first use, so an unused "
        "one costs nothing. An explicit `import` still returns the same object. Each module's exact "
        f"signatures come from `api('<name>')` / `help(<name>.<fn>)`, never from here. Modules: "
        f"{mods}. Also import-ready (these you DO `import`): {libs}."
    )


def credentials_note() -> str:
    """The external-credential sentence for the instructions, generated from
    `registry.credentialed()` so a credentialed service is declared in exactly
    one place (the registry) and this list can never drift from the probes."""
    needs = "; ".join(f"`{name}` ({cred.service})" for name, cred in registry.credentialed())
    return (
        f"Some bundled tooling calls an external service and needs a credential: {needs}. "
        "A call with a missing credential fails immediately with the remedy; check them all at "
        "once with `ix-mcp requirements` (the server also reports each one on stderr at startup)."
    )


# --- shared rules: reused by the kernel guide AND by tool descriptions ---

NAMESPACE = (
    "The namespace persists across calls, so variables, functions, classes, and imports you "
    "define stay defined and are reusable by every later call: define a helper once and call it "
    "again next turn. Bind expensive or large outputs to names (`out = await sh(...)`, "
    "`df = ...`) instead of only printing or returning them, so later calls can inspect, filter, "
    "or pass that same object to `read` without recomputing."
)

JOBS = (
    "Each call runs as an async task and waits up to `budget` seconds; if the work is still going "
    "it keeps running in the background and the call returns a job handle. Background jobs live "
    "in the `jobs` dict, so manage them with more python_exec: `jobs['ab12']` to inspect, `await "
    "jobs['ab12']` to wait (it yields the run's value), `jobs['ab12'].cancel()` to stop, "
    "`jobs['ab12'].done()` to test if it has finished, `[j for j in jobs.values() if "
    "j.running()]` to list. `budget` is how long the run holds the one shared shell channel "
    "before it backgrounds, so keep it small and poll: do NOT pass a huge budget to sit on a "
    "long `await jobs['ab12']` in the foreground — it blocks every other call for that whole "
    "time and is capped server-side anyway. Let the work background, then re-await or poll "
    "`.done()` in a later cell."
)

PAGING = (
    "Every run is kept in `jobs`, so a truncated reply is never lost: page the run with "
    "jobs['<id>'].tail(n) / .head(n) / .slice(a, b) / .grep('pat') / .lines(a, b), or read "
    "jobs['<id>'].output (stdout) and, once it has finished, jobs['<id>'].result (the value: "
    "it raises while the job is still running rather than return a misleading None, so `await "
    "jobs['<id>']` to wait for it; `.result` and `.result()` both work, and a finished Result's "
    "`.text` is its rendered text). Prefer assigning the result to a variable before producing a "
    "small final expression, e.g. `log = await sh(...); log[-4000:]`, so both the named object and "
    "the pageable job survive for follow-up calls; history() lists recent runs."
)

BLOCKING = (
    "Many jobs run at once and none blocks the others, but only if you never block the one shared "
    "event loop. Any synchronous wait (`subprocess.run`, `time.sleep`, `requests`, a long "
    "CPU-bound numpy op) freezes the WHOLE kernel: every other job and even your own next "
    "status-check cell stall behind it until it returns. So a blocking call MUST be made "
    "non-blocking: wrap it in `await asyncio.to_thread(...)`, or prefer an async API (`httpx`, "
    "the bundled `nu(...)` for a structured command and `sh(cmd)` for an external one instead of "
    "`subprocess.run`; the search helpers `grep`/`find`/`spotlight` shell out too, so `await` "
    "them), and run anything slow as a background job you "
    "poll, never inline. To shell out, reach for `sh()` rather than a hand-rolled "
    "`asyncio.create_subprocess_exec/_shell` + `communicate()`: `sh()` runs the child in its own "
    "session and enforces a timeout with a process-group kill, so it cannot sit in `running` "
    "forever when the command has finished but a child left the merged stdout pipe open (a "
    "`communicate()` that never returns) — the exact hang a raw async subprocess gives you."
)

RESULT_CONTRACT = (
    "Cells behave like a notebook: the last expression is the result, whatever its type (`2+2` "
    "returns 4, `df` returns the styled table with compact CSV to you, a string returns "
    "verbatim, a dict/list renders as a table). Prefer returning a Polars DataFrame for "
    "structured facts you expect to inspect, sort, filter, or show on the dashboard. Anything "
    "the cell printed comes back with "
    "it. A cell whose last statement is None (an assignment, a side-effecting call) returns its "
    "stdout, or a quiet ok. `yield` streams: each yielded value reaches both the human and you "
    "the moment it is produced, so yield as you go to report progress and partial results. "
    "`Result` is the opt-in for splitting the two views, `Result(user_html=..., llm_result=..., "
    "llm_images=...)`, when the human should see something rich that you should not pay tokens "
    "for (note: an explicit Result suppresses the automatic stdout echo; page "
    "jobs['<id>'].output instead). For reusable Python in the kernel, write explicit "
    "annotations at function and data boundaries. For package Python edits, run the repo's "
    "type-checking entry point when one exists, and do not treat an untyped compile-only "
    "check as equivalent."
)

# --- kernel guide only ---

INTRO = (
    "Run Python on one shared, persistent kernel with `python_exec`."
)

SESSION = (
    "First, call `session_set_name` with a short label for what you are working on. "
    "Acting tools are blocked until this MCP session is explicitly named. Every run "
    "you make is grouped under this session on the live dashboard, and a human may "
    "be watching several agents at once; a clear name is how they tell yours apart. "
    "It defaults to the connecting client and working directory (e.g. `claude-code · index`), "
    "which is ambiguous once agents share a repo, so name it. Then call `topic_set` "
    "before the first `python_exec` call and whenever you switch phases. A topic "
    "groups a handful of related runs under one fold in the dashboard, so use labels "
    "like `inspect diff`, `patch sidebar`, or `validate build`, not one topic per "
    "call. Also pass a one-line `intent` on every `python_exec` (it is required): "
    "the intent titles the run's card, so the board reads as a list of intents, not raw code."
)

PR_WATCH = (
    "For pull requests, use `pr_watch` instead of a hand-written polling loop. It creates a "
    "live PR resource under the current task, shows each required check or action with elapsed "
    "time, enables auto merge by default, and notifies the CLI when the PR merges, fails, or "
    "times out."
)

DISCOVER = (
    "`api()` is your reference (always in the namespace, no import): it lists every helper — the "
    "kernel builtins and each bundled module's public surface — with its live signature and a "
    "one-line summary. Call `api()` to see what exists, `api('grep')` to filter by name/summary/"
    "module, and `help(grep)` for a function's full doc. Take a name or a parameter from "
    "`api()` / `help()` rather than guessing: these instructions deliberately never restate "
    "signatures (so they cannot drift from the code), which makes the catalog the source of truth."
)

NO_SHELL = (
    "Do NOT hand-roll shell through Python: never `subprocess.run`, `os.system`, or "
    "`asyncio.create_subprocess_exec` for `ls`/`cat`/`grep`/`find`/`rg`/`fd` or any command whose "
    "output you parse. A bare `subprocess.run` is concretely worse, not just off-style: it runs "
    "synchronously on the kernel's one event loop and freezes every other job until it returns, "
    "and its piped output arrives corrupted (ANSI color codes get interleaved into the text, "
    "silently mangling and truncating the very tokens you parsed). The bundled tooling replaces "
    "it: for filesystem work `view.ls`/`view.tree` to list, `view.cat`/`view.edit(path, old, "
    "new)` to read and edit, `await grep(pattern)` / `await find(...)` to search (they wrap "
    "ripgrep/fd, run OFF the loop as a separate timeout-bounded process a runaway can't wedge, "
    "respect `.gitignore`, and return composable polars frames — `.filter`/`.sort`/`.group_by`/"
    "`.head` — so the human gets a styled table and you get a clean, uncorrupted frame); on "
    "macOS `await spotlight(query)`; for meaning-based recall across a corpus `import search`."
)

NU = (
    "For everything else a shell would do: running a command and shaping its output, a "
    "pipeline, listing/filtering/transforming, reaching into files or the web, reach for `nu` "
    "FIRST, before `sh`. `await nu(\"ls | where size > 1kb | sort-by size\")` runs a real "
    "nushell pipeline and every result comes back as a Polars DataFrame, structured end to end "
    "(`ls`, `ps`, `sys`, `open Cargo.toml`, `from csv`, `http get`, `where`, `group-by`, "
    "`select`) — no jq/awk/sed/cut text munging and no scraping columns out of a text dump. A "
    "record is one row, a scalar a one-cell `value` column; dates and durations arrive as real "
    "Datetime/Duration columns and filesize as bytes, so you filter and sort on typed values, "
    "not strings. For `gh ... --json`, clear color forcing before `from json`: "
    "`with-env {NO_COLOR: \"1\" CLICOLOR: \"0\" CLICOLOR_FORCE: \"0\" FORCE_COLOR: \"0\"} "
    "{ gh pr view 1 --json state | complete | get stdout | from json }`. "
    "The engine is embedded and PERSISTENT, a REPL: a `let`, a `def`, or a `cd` in "
    "one call is visible to the next, so bind an expensive fetch once (`let data = http get "
    "...`) and query it across calls; `nu.reset()` clears that state. Pipe a polars frame you "
    "already have THROUGH a pipeline with `await nu(\"where a > 1 | sort-by a\", input=df)`. Use "
    "`await nu.value(code)` when you want the plain Python value (a scalar, a list, a dict) "
    "rather than a frame. A failing pipeline raises `NuError` carrying nushell's own diagnostic "
    "(span + 'did you mean'), so read it and fix the pipeline. It evaluates off the event loop "
    "(tokio's blocking pool), but its `timeout` only interrupts BETWEEN pipeline elements and "
    "can't kill an external it already spawned (whereas `sh` group-kills on timeout), and calls "
    "against the shared engine run one at a time — so keep a genuinely long external in `sh`."
)

SH = (
    "`sh` is the escape hatch for what `nu` (and `nix`) should not do: running a real external "
    "binary (`git`/`gh`/`cargo` and other writes and side effects; for `nix` use the `nix` module "
    "above, not `sh(['nix', ...])`), a long-running command, or one whose raw text/log you want "
    "verbatim. It runs the child off the loop in its own "
    "session, streams into the job's pageable output, enforces a timeout with a process-group "
    "kill (so it can't hang in `running` after the command finished but a child held the pipe "
    "open — the exact failure a raw async subprocess gives you), and preserves clean color. To "
    "run elsewhere pass `cwd=`, never a `cd X && ...` prefix; one command per call, never "
    "`cmd1; echo ===; cmd2` chains scraped apart with string splitting. Prefer DATA over text "
    "even here: when the CLI speaks JSON (`gh --json`, `cargo metadata`, `kubectl -o json`) use it "
    "and parse the Output with `.json()` / `.jsonl()` / `.df()` (a polars frame ready to filter "
    "and render). `sh` takes a shell string (`await sh('git status --short')`) or an argv list "
    "(`await sh(['git', 'commit', '-m', msg])`) that bypasses shell parsing; `await sh.zsh(...)` "
    "only when you intentionally need zsh syntax. Never pass prose through shell quoting: "
    "backticks in a string command run as command substitution even inside a Python repr'd "
    "string (this is how a commit message once executed `ix-mcp dashboard` and spliced its URL "
    "into the message), and a repr'd multi-line string loses its newlines — for any prose "
    "argument (a commit message, a PR body) use the argv-list form so it bypasses shell parsing, "
    "or write it to a temp file and `git commit -F <file>`."
)

NIX = (
    "For any `nix` command, use the bundled `nix` module — NEVER `sh(['nix', ...])` or "
    "`nu('nix ...')`. `await nix.run(['build', '.#foo'])` (or the shorthand `await "
    "nix.build('.#foo')`) runs the build through the nix-web-monitor emitter and, for free, "
    "publishes a LIVE build-tree pane to the dashboard — every derivation with its phase and "
    "status, in-flight fetches with progress bars, failures highlighted — that updates as the "
    "build runs and self-closes when it finishes. The returned handle exposes `.ok`, `.errors`, "
    "and `.builds` (a polars frame), so branch on the outcome the same way as `sh().ok`. `await "
    "nix.eval('.#x', apply='...')` returns a native Python value without hand-quoting a Nix "
    "function through the shell, and `await nix.attrs('.')` catalogs a flake's buildable outputs "
    "as a frame. Run a long build as a background job and sample the handle between turns. Drop to "
    "`sh` for nix ONLY when you need its raw stdout verbatim (e.g. `nix eval --raw`)."
)

VERIFY = (
    "Verify a change by its actual effect, not by a proxy: when you change "
    "something whose result a static check cannot see — an interactive UI, a "
    "rendered page, a runtime behaviour — exercise it and observe the outcome "
    "(drive a real browser with the bundled `browser` module — `await "
    "browser.goto(url)`, then `browser.shot()` / `browser.vdom()` — run the path, "
    "diff the live state) BEFORE reporting it done. Reach for `browser`, not raw "
    "`playwright`: it keeps one cached connection on the kernel loop, opens a "
    "VISIBLE window the human can watch, and publishes the page as a live "
    "dashboard resource; calling `async_playwright().start()` yourself gets none "
    "of that. A green type-check or linter is necessary but not sufficient: 'it "
    "compiles' is not 'it works', and 'the tab switches in the source' is not "
    "'the tab switches on screen'."
)

AUTOMERGE = (
    "By default, merge a pull request you open yourself rather than handing it back: once its "
    "required checks are green, resolve your own review threads and merge it (through the merge "
    "queue when the repo sets one). Leave a PR open only when the change genuinely needs human "
    "sign-off or the user asked you to."
)

HTML = (
    "htpy (build HTML in Python with `div(class_='x')[...]`; it auto-escapes every text node and "
    "attribute, so use it instead of f-string HTML, which is where escaping gets forgotten; an "
    "htpy element renders directly through `cells.add(el)`/`Result.of(el)`, so do not wrap it in "
    "`IPython.display.HTML` or stringify it). When you hand-build HTML, drive colors from CSS "
    "custom properties with a `@media (prefers-color-scheme: dark)` override (never hard-coded "
    "light-only colors), so it follows the viewer's OS theme — the dashboard is dark by default."
)

OUTPUT_HTML = (
    "By default, when you give the human an output, write it to an HTML file and then open it: build "
    "the page with htpy, write it to a file (`from pathlib import Path; Path('out.html').write_text(str(el))`), and open it for "
    "the viewer with `await sh(['open', 'out.html'])` so it lands in their browser. Reach past a plain "
    "text answer to this rendered page for anything worth seeing."
)

POLARS = (
    "Prefer Polars for any tabular data: return a DataFrame (or `Result.of(df)`) and the human "
    "gets the styled HTML table for free while you get the frame as compact, untruncated CSV — so "
    "you never hand-build a table and a wide/long-stringed frame is never clipped to you. Use "
    "`pl`; pandas is not bundled. Even key/value data — environment variables, a config dict, "
    "counts — is tabular: return a two-column DataFrame, never a `\\n`-joined string or a printed "
    "dict; a plain list of scalars returns as a one-column frame too, so `Result(items)` just "
    "works. Nested data renders recursively: a dict of dicts (or a frame with struct/list columns) "
    "shows each value as a nushell-style nested sub-table, so prefer one frame with struct/list "
    "columns over a `{label: blob}` dict that collapses each value to a clipped repr — or add each "
    "item as its own `cells.add`. To QUERY JSON or nested data, decode a JSON-string column with "
    "`str.json_decode()` into a Struct, then reach into it: `pl.col('s').struct.field('x')` for one "
    "field, `.struct.unnest()` to expand every field into its own column. There is no single "
    "'does any field match' op — reduce across fields with `pl.any_horizontal(...)` / "
    "`pl.all_horizontal(...)` over the unnested struct (e.g. "
    "`pl.any_horizontal(pl.col('s').struct.unnest().is_null())` is 'any field is null'); add "
    "`.fill_null(False)` inside the reducer when a missing field should count as no-match."
)

RESULT_SPLIT = (
    "A Result splits the human view from your view: the dashboard renders `user_html` for the "
    "human while your tool result gets `llm_result` text plus any `llm_images`, so a big rendered "
    "view never costs you tokens and you can hand yourself back images."
)

RESULT_VARIANTS = (
    "Use the shortcuts: `Result.text('done')` (same text to both), `Result.ok('what happened')` "
    "(a quiet confirmation for a side-effecting cell), `Result.of(df)` (render any value richly "
    "for the human, its repr to you), or `Result(user_html=..., llm_result=..., llm_images=[fig, "
    "png_bytes])`. To show several values at once, pass them as separate args "
    "(`Result(repr(hits), hits.df)`) or in one list/tuple: each renders with its "
    "OWN view, stacked, so a DataFrame stays a real table and is never flattened "
    "into a cell of some outer frame."
)

READABLE = (
    "Write the cell as readable, idiomatic Python, not a cramped one-liner: the dashboard shows "
    "your source verbatim, so a human reads it. Use real statements over several lines, bind "
    "intermediate results to clearly named variables, and break a long comprehension or chained "
    "call across lines instead of nesting it inside a `str(...)` or a `Result(...)` call. Let the "
    "final `Result(...)` name what it returns rather than wrap one dense expression."
)

CHANNEL = (
    "This server is also a Claude Code channel (research preview). When the client session was "
    "launched with the channel enabled (`claude --dangerously-load-development-channels "
    "server:<name>`), kernel code can push events into the running agent session with `await "
    "notify(content, **meta)`: each event arrives in the session as <channel source=\"...\" "
    "key=\"val\">content</channel>, with each meta kwarg a tag attribute (identifier keys only). "
    "Delivery is fire-and-forget — a session without the channel enabled drops events silently — "
    "so never treat a notify as confirmed-read. Interactive resources close the loop: "
    "`register_resource(render=..., actions={'name': handler})` serves the HTML with "
    "`ix.act(name, payload)` (queues the payload for the named in-kernel handler) and "
    "`ix.events(fn)` (subscribes the page to handler results, errors, and your replies) "
    "pre-wired. Call `notify(..., resource=<id>)` in every action handler by default: without "
    "it the page↔kernel loop runs silently and you only learn the human acted by polling kernel "
    "state. Skip it only when a click is purely page-local (a filter toggle, a re-render). "
    "When a <channel> tag carries a `resource` attribute, answer it with the `reply` tool, "
    "passing that resource id — your transcript output never reaches the page. For any "
    "non-trivial UI, author a real Svelte 5 component instead of hand-rolled HTML/JS strings: "
    "`await svelte.component(\"Board.svelte\", id=..., state=..., actions=...)` (module "
    "`svelte`) compiles it to one self-contained bundle; the component imports "
    "`{ data, act } from 'ix'` and re-renders reactively from the dict each handler returns, "
    "so there is one renderer and kernel state stays the single source of truth."
)

CELLS = (
    "Three dashboard panes show the session live: every running/finished run under executions, "
    "every live view (a terminal, a widget) under resources, and your curated highlight reel "
    "under cells; its address is the `DASHBOARD_URL` value in the namespace (read the variable — "
    "there is no `dashboard()` function to call). Answer THROUGH cells by default: the cells pane "
    "is what the human reads as the answer, so put any result worth seeing there with "
    "`cells.add(value, title=...)` (a DataFrame, a figure, a `view` render, an htpy "
    "element) rather than leaving it only in your tool text. Treat cells as the FINAL "
    "PRESENTATION of the CURRENT state, not an append-only log: add the most important results "
    "with `cells.add(value, id=..., title=...)` (a stable `id=` makes a re-run replace that cell "
    "in place instead of stacking a duplicate), and prune stale ones as the state moves on — "
    "`cells.set(key, value)` to replace in place, `cells.remove(key)` to drop one, "
    "`cells.clear()` to start over — so the page always reflects where things stand now."
)


# --- tool descriptions (composed into @mcp.tool(description=...)) ---

PYEXEC_INTRO = (
    "Run Python on the shared persistent kernel. Waits up to `budget` seconds; if the code is "
    "still running it keeps going in the background as jobs['<id>'] and this returns a job handle "
    "(inspect / await / cancel it via more python_exec on the `jobs` dict)."
)

SEE_INSTRUCTIONS = (
    "The server instructions cover the rest — the bundled tooling (grep / find / view / nix / "
    "fleet / polars / htpy), how to find and read things, and how to curate the dashboard's cells."
)

READ = (
    "Read a file (or a kernel value) into YOUR context WITHOUT spamming the human's dashboard: "
    "the full text comes back to you, while the dashboard shows only a one-line note (path, line "
    "span, size). Use this instead of `cat` / `view.cat` or printing a big value through "
    "python_exec whenever the content is for you to read, not for the human to look at — a normal "
    "cell's result streams to BOTH audiences, so it would flood the dashboard. `target` is read "
    "as a file when it names an existing file, otherwise it is evaluated as a Python expression "
    "in the kernel namespace (e.g. `jobs['ab12'].output` to page a job, or a variable you bound "
    "earlier); an expression whose value is a string naming an existing file reads that file "
    "too. Pass `start` / `end` for a 1-based inclusive line range."
)

TRACE = (
    "Dump the kernel's current Python stack for every thread. Works even when a cell has wedged "
    "the kernel by blocking its event loop with a synchronous call (subprocess.run, time.sleep, "
    "requests, a long CPU op): the dump is captured via a faulthandler signal, not the execute "
    "channel, so it returns while the loop is still frozen. Use it to see WHERE a wedged or slow "
    "cell is stuck, then fix the blocking call (wrap it in `await asyncio.to_thread(...)` and "
    "background it)."
)

REPLY = (
    "Send a message to the page behind an interactive resource. Use it to answer a channel event "
    "that carries a resource attribute (<channel resource=\"...\">): pass that resource id and "
    "your text, and the page receives it on its live event feed (`ix.events`). The page's viewer "
    "reads the page, not this session — anything you want them to see must go through this tool; "
    "your transcript output never reaches them. Fails when the resource is closed or unknown."
)


# --- appended once the dashboard has bound a port ---

_DASHBOARD_URL_NOTE = (
    "This session's live dashboard (every running job, its output, and your curated cells) is at "
    "{url} -- ALWAYS give the human this link in your very first reply of the session, before or "
    "alongside your first answer, so they can watch the work unfold live; never make them ask for "
    "it. It is also the `DASHBOARD_URL` variable in the kernel namespace."
)


def dashboard_note(url: str) -> str:
    """The live-dashboard sentence the CLI folds into the instructions once the
    dashboard has a URL (see tools.set_dashboard_url)."""
    return _DASHBOARD_URL_NOTE.format(url=url)
