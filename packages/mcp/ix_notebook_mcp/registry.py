"""Single source of truth for the bundled modules and namespace builtins.

The kernel runtime AND the server instructions both derive from this one table,
so a helper is declared in exactly one place. The kernel reads it for
``runtime._API_MODULES`` / ``_API_BUILTINS`` (which feed the live ``api()``
catalog) and for the startup pre-import list; the server reads it for the module
index in the instructions (:func:`guide.modules_index`). Because the per-function
signatures come from live introspection in ``api()`` and never from prose, the
instructions cannot drift from the code: add a row here and it shows up in
``api()``, gets pre-imported if marked, and is listed in the instructions -- with
no signature copied by hand anywhere.

A tagline is a one-line *capability*, never a signature or parameter list (those
belong to ``api()`` / ``help()``).
"""

from __future__ import annotations

import dataclasses


@dataclasses.dataclass(frozen=True)
class Credential:
    """An external credential a module's (or library's) calls need to succeed.

    Declarative and probe-only: :mod:`.requirements` checks the env vars and the
    token file locally (existence only -- no file reads, no network, never a
    secret's value), so the report is free and safe to run anywhere. The module
    itself still owns resolution at call time; these fields mirror that module's
    documented sources so the report can say where a credential WOULD come from
    and exactly how to get one.
    """

    service: str  # how the service reads in messages ("Mixedbread")
    env: tuple[str, ...] = ()  # env vars that satisfy it, in resolution order
    token_path: str | None = None  # "~/..." token file a login flow writes
    login: str | None = None  # full clause for the login route ("run `mgrep login`")
    url: str | None = None  # where a human gets a key


@dataclasses.dataclass(frozen=True)
class Module:
    """A first-party bundled module that ``api()`` catalogs by introspection."""

    name: str
    tagline: str
    preimport: bool = False  # bound into the namespace at startup (no import needed)
    credential: Credential | None = None  # external service its calls depend on


@dataclasses.dataclass(frozen=True)
class Builtin:
    """A name always present in the kernel namespace (installed by runtime.install)."""

    name: str
    tagline: str


# The bundled modules, in the order they should appear in the catalog and the
# instructions. Pre-imported ones are usable with no ``import`` (like the builtins).
MODULES: tuple[Module, ...] = (
    Module(
        "view",
        "ls / tree / grep / find / cat / head / json / diff / edit -- directories and searches "
        "as polars frames, files as syntax-highlighted views",
        preimport=True,
    ),
    Module(
        "nu",
        "structured shell, the ONE shell-out path (an embedded, persistent nushell "
        "engine): run a pipeline and get a polars DataFrame back, always -- "
        '`await nu("ls | where size > 1kb | sort-by size")`, `open Cargo.toml`, `from csv`, '
        "`http get`; run an external binary with `^cmd` (`^git status`, `^gh pr list --json .. "
        "| from json`); `let`/`def`/`cd` persist across calls like a REPL; `input=df` pipes a "
        "frame through a pipeline; `nu.value(code)` returns the plain Python value; a failure "
        "raises NuError carrying nushell's own diagnostic. Replaces jq/awk/sed text munging and "
        "the retired `sh`",
        preimport=True,
    ),
    Module(
        "nix",
        "run a nix build with a live dashboard build-tree pane; the returned BuildRun exposes "
        "`.ok` / `.errors` / `.builds` (a polars frame). `nix.eval()` returns a flake value and "
        "`nix.attrs()` catalogs the flake's buildable attrs; `nix.parse()` folds a captured "
        "internal-json log into polars frames",
    ),
    Module("fleet", "async polars SSH fan-out across hosts (`read_ndjson` / `scan`)"),
    Module(
        "mesh",
        "tailnet mesh of live ix-mcp servers, zero config: `await mesh.peers()` is one polars "
        "row per reachable server (host, version, named sessions, dashboard URL) discovered "
        "via tailscale; `await mesh.sessions()` flattens to one row per (host, session)",
        preimport=True,
    ),
    Module(
        "search",
        "meaning-based recall across the fleet corpus (code + agent/shell history): "
        "`await search.semantic(q, since='7d', compact=True)` / `grep(pattern)` / "
        "`recent(source=['shell'], since='6h')` newest-first. Each is async and "
        "returns a polars frame (one row per hit, compose `.filter`/`.group_by`/"
        "`.head`) with timestamp/user/host/session_id provenance columns",
        # Mirrors the resolution order owned by packages/search/mixedbread/src/auth.rs
        # (env key, else the `mgrep login` token), which the bundled module
        # reaches through search-py.
        credential=Credential(
            service="Mixedbread",
            env=("MXBAI_API_KEY",),
            token_path="~/.mgrep/token.json",  # noqa: S106 -- path to token file, not a hardcoded secret
            login="run `mgrep login`",
            url="https://www.mixedbread.com",
        ),
    ),
    Module(
        "astlog",
        "Datalog over tree-sitter ASTs: tree-sitter query matches become relations, rules join "
        "them (position, text, recursion), `(lint ...)` rows become findings, rewrites turn rows "
        "into edits. Results come back as polars DataFrames: `astlog.query` (dict of frames), "
        "`scan` (lint findings), `suppressed` (what `astlog-ignore` hides + the comment), "
        "`fixes`; `fix` returns a diff",
    ),
    Module(
        "flecs_query",
        "parse the Flecs Query Language without a flecs world: expression to AST dicts, "
        "canonical form, syntax verdicts (`flecs_query.parse` / `canonicalize` / `validate`)",
    ),
    Module("tui", "drive and snapshot a terminal program; renders as HTML"),
    Module(
        "screen",
        "native macOS desktop control: capture the screen, a region, or one app's window "
        "(`screen.capture(app=...)`), move/click the mouse, type, and manage apps (macOS only)",
    ),
    Module(
        "vmkit",
        "boot and drive a macOS/Linux guest VM fully off-screen and screenshot its display "
        "(macOS only)",
    ),
    Module(
        "imessage",
        "read Messages and Contacts into polars and send iMessages "
        "(`imessage.messages()` / `chats()` / `send()`) (macOS only)",
    ),
    Module(
        "maps",
        "native macOS places & geocoding, each an async call returning a polars frame: "
        "`await maps.nearby(query, lat, lng)` searches places near a point (MapKit), and "
        "`await maps.geocode(address)` / `await maps.reverse_geocode(lat, lng)` convert "
        "address <-> coordinates (CoreLocation). No API key or cost — Apple's on-device "
        "stack (macOS only)",
    ),
    Module(
        "ghostty",
        "drive the Ghostty terminal via its AppleScript dictionary: read every open "
        "surface into polars (`await ghostty.surfaces()` -> id/tty/pid/cwd/name), and "
        "close/focus/activate one by tty or id. `await ghostty.close_me()` shuts the "
        "window this very session runs in -- the end-of-task move once fully done "
        "(macOS only)",
    ),
    Module(
        "iphone",
        "drive a physical iPhone/iPad (USB or Wi-Fi) via pymobiledevice3: list devices/apps into "
        "polars (`iphone.devices()` / `apps()`), `screenshot()` a developer-mounted device to a PIL "
        "image, `tap`/`swipe`/`launch`, and mount the Developer Disk Image. Developer commands "
        "need a root `tunneld` daemon (start it explicitly with `iphone.start_tunneld(sudo=True)`) "
        "plus a device with Developer Mode on; works cable-free once the device is paired and "
        "network-enabled (see the iphone-control skill)",
    ),
    Module(
        "tasks",
        "generate and read the task-graph demo's SQLite DAG (`tasks.seed` / `load` / `frame`)",
    ),
    Module(
        "mcp_client",
        "call any MCP server's tools from Python: `await mcp_client.connect(url_or_command)` "
        "returns a live server whose `.tools` is a polars frame and whose `await srv.call(tool, "
        "**args)` runs a tool (stdio or streamable-HTTP; bearer-token / header auth, plus "
        "interactive OAuth + PKCE with cached, auto-refreshed tokens for remote servers)",
    ),
    Module(
        "worktree",
        "do risky or parallel work on a throwaway branch in its own checkout, leaving the main "
        "tree untouched",
    ),
    Module(
        "browser",
        "drive a running browser over CDP with Playwright (connects to the standard debug port "
        "9222 by default); `browser.vdom()` returns a clean, filtered virtual DOM -- a "
        "machine-readable map of where every control, landmark and heading is (role, name, box, "
        "CSS selector) -- so you can act without a screenshot; `browser.read()` is a lighter "
        "text-first readout and `browser.shot()` renders a screenshot inline",
    ),
    Module(
        "google_auth",
        "Google for your own account: read and send Gmail, and manage Calendar, over the "
        "official googleapiclient (`google_auth.gmail()` / `.calendar()`); "
        "`await google_auth.login()` signs in through your browser and `status()` / `logout()` "
        "manage the grant. Incognito sessions only (a personal mailbox never reaches a shared room)",
        # The bundled `gcal` binary owns the grant; the stored refresh token
        # lives at this documented path (mode 0600).
        credential=Credential(
            service="Google",
            token_path="~/.config/google/token.json",  # noqa: S106 -- path to token file, not a hardcoded secret
            login="call `await google_auth.login()` in a cell",
        ),
    ),
    Module(
        "x",
        "read recent X (Twitter) posts into polars by driving your logged-in browser: "
        "`await x.posts(\"@handle\")` / `await x.posts(\"home\")` / `await x.posts(\"#tag\")` / a thread URL, "
        "scrolled until it has `limit` tweets (one row each, with author, time, text and counts). Reads the signed-in account's personal feed, so incognito sessions only (a shared room never sees your timeline)",
    ),
    Module(
        "slack",
        "read Slack channels, messages, and threads into polars, send messages, and search "
        "(`await slack.channels()` / `messages(channel)` / `thread(channel, ts)` / `send(channel, text)` / `search(query)`); "
        "`slack.login(token)` stores your user token (mode 0600); `status()` / `logout()` manage it. "
        "Incognito sessions only (personal Slack data never reaches a shared room)",
        # Mirrors slack._token()'s documented resolution order; the smoke test
        # pins these against the module's own constants so they cannot drift.
        credential=Credential(
            service="Slack",
            env=("SLACK_USER_TOKEN", "SLACK_TOKEN"),
            token_path="~/.config/slack/token",  # noqa: S106 -- path to token file, not a hardcoded secret
            login="call `slack.login(token)` in a cell",
            url="https://api.slack.com/authentication/token-types#user",
        ),
    ),
    Module(
        "beeper",
        "read chats and messages across every network (WhatsApp, Telegram, Signal, iMessage, "
        "Discord, Slack, X, ...) into polars from the local Beeper Desktop API, search, and send "
        "(`await beeper.accounts()` / `chats()` / `messages(chat_id)` / `search(query)` / "
        "`send(chat_id, text)`); `beeper.login(token)` stores your access token (mode 0600), "
        "`await status()` / `logout()` manage it. Incognito sessions only (personal chats never "
        "reach a shared room)",
        # Mirrors beeper._token()'s resolution order; the requirements smoke test
        # pins these against the module's own constants so they cannot drift.
        credential=Credential(
            service="Beeper",
            env=("BEEPER_ACCESS_TOKEN", "BEEPER_API_TOKEN"),
            token_path="~/.config/beeper/token",  # noqa: S106 -- path to token file, not a hardcoded secret
            login="call `beeper.login(token)` in a cell",
            url="https://developers.beeper.com/desktop-api/auth",
        ),
    ),
    Module(
        "linear",
        "Linear issue tracker over GraphQL using LINEAR_API_KEY: "
        "`await linear.issue(id)` / `issue_update(id, **fields)` / "
        "`issue_create(team, title, **fields)` / `project_create(name, teams, **fields)`",
        credential=Credential(
            service="Linear",
            env=("LINEAR_API_KEY",),
            url="https://linear.app/settings/account/security",
        ),
    ),
    Module(
        "notion",
        "Notion pages, databases, and blocks over the REST API using NOTION_API_KEY: "
        "`await notion.search(query)` / `page(page_id)` / `blocks(block_id)` / "
        "`db_query(database_id, filter=, sorts=)` / `page_create(parent, properties)` / "
        "`blocks_append(block_id, children)` / `page_update(page_id, properties)`",
        credential=Credential(
            service="Notion",
            env=("NOTION_API_KEY",),
            url="https://www.notion.so/my-integrations",
        ),
    ),
)

# Always-present namespace builtins (installed by runtime.install; no import).
BUILTINS: tuple[Builtin, ...] = (
    Builtin("Result", "split a cell's value into the human view and your view; a cell must end with or yield one"),
    Builtin("cells", "curate the dashboard's highlight reel (`cells.add` / `set` / `remove` / `clear`)"),
    Builtin("session", "this session's dashboard identity — set `session.name = '...'` first so a human can tell your runs apart"),
    Builtin("jobs", "the background-run registry (inspect / await / cancel / page each run)"),
    Builtin("history", "list recent runs"),
    Builtin("doc", "the signature + docstring of any object, returned as a Result (help() only prints and returns None)"),
    Builtin("resources", "the live, self-updating views (a terminal, a widget)"),
    Builtin("register_resource", "publish a live Resource to the dashboard"),
    Builtin(
        "ask",
        "pop a window asking the human and await the reply: `await ask(\"prompt\")` / "
        "`ask(\"prompt\", choices=[...])` / `ask(\"prompt\", fields=[...])`. Blocks on the "
        "human, so it exceeds your budget and backgrounds -- read it back with `await jobs['<id>']`",
    ),
    Builtin(
        "Input",
        "the general input primitive behind `ask`: drop its `.script` into any resource HTML, "
        "have the markup call `ixSubmit(payload)`, then `await` the submission (or `async for`)",
    ),
    Builtin(
        "notify",
        "push a channel event into the connected agent session (Claude Code channels): "
        "`await notify('build failed', severity='high')`; each kwarg becomes a <channel> tag "
        "attribute (identifier keys only). Fire-and-forget: a session without the channel "
        "enabled drops it silently",
    ),
    Builtin(
        "watch_pr",
        "watch a GitHub PR as a live resource, show required checks with elapsed time, enable "
        "auto merge by default, and notify when it merges, fails, or times out",
    ),
    Builtin("api", "the live catalog of every helper, as a polars frame (`api('grep')` to filter)"),
    Builtin(
        "read_stats",
        "this session's cumulative file-read counters ({total_reads, redundant_reads}); a "
        "redundant read is a file re-read with byte-identical content -- check your own "
        "redundancy rate (KPI: redundant/total < 1%)",
    ),
    Builtin(
        "grep",
        "content search backed by ripgrep (process-isolated + timeout, so it can't wedge the "
        "kernel): `await grep(pattern)` -> a polars frame, one row per match "
        "(path/line_number/col/match/line); searches the cwd, respects .gitignore by default",
    ),
    Builtin(
        "find",
        "file search backed by fd: `await find(ext='py')` -> a polars frame "
        "(path/name/type/size/mtime); searches the cwd, respects .gitignore by default; process-isolated + timeout",
    ),
    Builtin(
        "spotlight",
        "full-text + metadata search backed by macOS Spotlight (mdfind), macOS only: "
        "`await spotlight(query)` -> a polars frame (path/name/type/size/mtime)",
    ),
    Builtin("asyncio", "stdlib asyncio, pre-bound: `asyncio.ensure_future` / `sleep` with no import"),
    Builtin("json", "stdlib json, pre-bound: parse a CLI's --json output with no import"),
    Builtin("pl", "polars, pre-bound: build/transform DataFrames with no import"),
    Builtin("DASHBOARD_URL", "this session's live dashboard URL"),
)

@dataclasses.dataclass(frozen=True)
class Library:
    """A bundled third-party library with no first-party ``api()`` surface."""

    name: str
    credential: Credential | None = None  # external service its calls depend on
    note: str | None = None  # one-line steer shown in the instructions' library list


# Standard third-party libraries that are bundled and import-ready. They have no
# first-party ``api()`` surface (use ``help()`` / their own docs); named here so
# the instructions list them in one place.
LIBRARIES: tuple[Library, ...] = (
    Library("numpy"),
    Library("polars"),
    Library("duckdb"),
    Library("httpx"),
    Library("matplotlib"),
    Library("pypdf"),
    Library(
        "playwright",
        # Agents reach for raw `async_playwright().start()`; steer them to the
        # `browser` module, which manages the connection on the kernel loop and
        # publishes the live dashboard resource.
        note="prefer the `browser` module — `browser.goto`/`shot`/`vdom` drive a "
        "visible browser on the kernel loop and publish a live dashboard resource; "
        "don't call `async_playwright().start()` yourself",
    ),
    Library(
        "exa_py",
        # The SDK is a thin client over the Exa REST API; no key is bundled,
        # the caller constructs `Exa(os.environ["EXA_API_KEY"])`.
        credential=Credential(
            service="Exa",
            env=("EXA_API_KEY",),
            url="https://dashboard.exa.ai/api-keys",
        ),
    ),
    Library(
        "cursor_sdk",
        # Cursor's official agent SDK: run the same agent as the Cursor IDE/CLI
        # (local or cloud) from a cell, e.g. Composer as a cheap delegated
        # codebase-search agent. No key is bundled; local runs also honor a
        # logged-in `cursor-agent`.
        credential=Credential(
            service="Cursor",
            env=("CURSOR_API_KEY",),
            url="https://cursor.com/dashboard",
        ),
    ),
)


def module_names() -> tuple[str, ...]:
    return tuple(m.name for m in MODULES)


def preimport_names() -> tuple[str, ...]:
    return tuple(m.name for m in MODULES if m.preimport)


def builtin_names() -> tuple[str, ...]:
    return tuple(b.name for b in BUILTINS)


def credentialed() -> tuple[tuple[str, Credential], ...]:
    """Every (module-or-library name, credential) pair, registry order.

    The single iteration the requirements report, the instructions sentence,
    and the startup yelling all build from, so a new credentialed service is
    declared in exactly one place.
    """
    return tuple(
        (entry.name, entry.credential)
        for entry in (*MODULES, *LIBRARIES)
        if entry.credential is not None
    )
