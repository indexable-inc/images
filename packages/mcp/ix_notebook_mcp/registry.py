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
        "nix",
        "run a nix build and get its internals as polars (`.events` / `.activities`) plus a live "
        "build DAG; `nix.attrs()` catalogs the flake's buildable attrs",
    ),
    Module("fleet", "async polars SSH fan-out across hosts (`read_ndjson` / `scan`)"),
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
        "sh",
        "shell out on the loop; the Output IS a Result (ANSI as HTML for the human, "
        "`.text`/`.code`/`.ok` for you), and `.json()`/`.jsonl()`/`.df()` parse a JSON-mode "
        "CLI straight to data / a polars frame: ask the tool for --json, never scrape TSV",
    ),
    Builtin("api", "the live catalog of every helper, as a polars frame (`api('grep')` to filter)"),
    Builtin(
        "grep",
        "content search backed by ripgrep (process-isolated + timeout, so it can't wedge the "
        "kernel): `await grep(pattern, root='.')` -> a polars frame, one row per match "
        "(path/line_number/col/match/line); respects .gitignore by default",
    ),
    Builtin(
        "find",
        "file search backed by fd: `await find(ext='py', root='.')` -> a polars frame "
        "(path/name/type/size/mtime); respects .gitignore by default; process-isolated + timeout",
    ),
    Builtin(
        "spotlight",
        "full-text + metadata search backed by macOS Spotlight (mdfind), macOS only: "
        "`await spotlight(query, root='~')` -> a polars frame (path/name/type/size/mtime)",
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
