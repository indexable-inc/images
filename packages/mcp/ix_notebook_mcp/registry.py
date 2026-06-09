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
class Module:
    """A first-party bundled module that ``api()`` catalogs by introspection."""

    name: str
    tagline: str
    preimport: bool = False  # bound into the namespace at startup (no import needed)


@dataclasses.dataclass(frozen=True)
class Builtin:
    """A name always present in the kernel namespace (installed by runtime.install)."""

    name: str
    tagline: str


# The bundled modules, in the order they should appear in the catalog and the
# instructions. Pre-imported ones are usable with no ``import`` (like the builtins).
MODULES: tuple[Module, ...] = (
    Module(
        "fff",
        "typo-tolerant file find + SIMD content grep; every result's `.df` is a polars frame",
        preimport=True,
    ),
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
    Module("search", "meaning-based semantic recall across an indexed corpus"),
    Module("tui", "drive and snapshot a terminal program; renders as HTML"),
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
    ),
    Module(
        "x",
        "read recent X (Twitter) posts into polars by driving your logged-in browser: "
        "`await x.posts(\"@handle\")` / `await x.posts(\"home\")` / `await x.posts(\"#tag\")` / a thread URL, "
        "scrolled until it has `limit` tweets (one row each, with author, time, text and counts). Reads the signed-in account's personal feed, so incognito sessions only (a shared room never sees your timeline)",
    ),
)

# Always-present namespace builtins (installed by runtime.install; no import).
BUILTINS: tuple[Builtin, ...] = (
    Builtin("Result", "split a cell's value into the human view and your view; a cell must end with or yield one"),
    Builtin("cells", "curate the dashboard's highlight reel (`cells.add` / `set` / `remove` / `clear`)"),
    Builtin("jobs", "the background-run registry (inspect / await / cancel / page each run)"),
    Builtin("history", "list recent runs"),
    Builtin("doc", "the signature + docstring of any object, returned as a Result (help() only prints and returns None)"),
    Builtin("resources", "the live, self-updating views (a terminal, a widget)"),
    Builtin("register_resource", "publish a live Resource to the dashboard"),
    Builtin("sh", "shell out on the loop; the Output IS a Result (ANSI as HTML for the human, `.text`/`.code`/`.ok` for you)"),
    Builtin("api", "the live catalog of every helper, as a polars frame (`api('grep')` to filter)"),
    Builtin("DASHBOARD_URL", "this session's live dashboard URL"),
)

# Standard third-party libraries that are bundled and import-ready. They have no
# first-party ``api()`` surface (use ``help()`` / their own docs); named here so
# the instructions list them in one place.
LIBRARIES: tuple[str, ...] = (
    "numpy",
    "polars",
    "duckdb",
    "httpx",
    "matplotlib",
    "playwright",
    "exa_py",
)


def module_names() -> tuple[str, ...]:
    return tuple(m.name for m in MODULES)


def preimport_names() -> tuple[str, ...]:
    return tuple(m.name for m in MODULES if m.preimport)


def builtin_names() -> tuple[str, ...]:
    return tuple(b.name for b in BUILTINS)
