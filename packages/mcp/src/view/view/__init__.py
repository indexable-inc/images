"""Pretty, composable views of files and search results for the ix-mcp kernel.

Bundled like ``tui``/``search`` so every session can ``import view`` with no
setup. The point: stop hand-rolling ``Result(user_html=...)`` for the reads and
listings an agent does constantly, and never shell out to ``ls``/``grep``/``cat``
(use ``fff``/``polars``/``pathlib`` instead, for structured, composable output).

Two flavors of view:

* The tabular helpers (:func:`ls`, :func:`tree`) return a plain
  ``polars.DataFrame``. They compose with the full polars API (``.filter()``,
  ``.sort()``, ``.join()`` ...) and render as the dashboard's styled HTML table,
  because the kernel installs a global ``polars.DataFrame._repr_html_`` built
  from :func:`df_html`. For content/file search use ``fff.grep`` / ``fff.find``
  (their results render as the same styled table and expose ``.df``).
* The file helpers (:func:`cat`/:func:`read`, :func:`head`, :func:`tail`,
  :func:`json`, :func:`diff`) return a :class:`Code`: a syntax-highlighted HTML
  render for the human plus the raw text as the agent's value.

Every helper gives the human a rich view and the agent concise text, with no
extra ``Result(...)`` boilerplate.
"""

from __future__ import annotations

import difflib
import html as _html
import json as _json
import os
import pathlib
import subprocess
from datetime import datetime

import polars as pl

# Heavy, rarely-relevant directories `tree` collapses (lists but does not descend
# into) unless ``all=True``: dependency installs, build output, vendored code, and
# caches. Dotted dirs (.git, .venv, .svelte-kit, ...) are already skipped by the
# hidden-entry rule, so this names only the non-dotted offenders that otherwise
# bury a project's real structure under thousands of files.
_NOISE_DIRS = frozenset(
    {
        "node_modules",
        "target",
        "build",
        "dist",
        "out",
        "result",
        "vendor",
        "venv",
        "coverage",
        "__pycache__",
    }
)


def _git_ignored(root: pathlib.Path, rels: list[str]) -> set[str]:
    """The subset of ``rels`` (paths relative to ``root``) that git ignores, via
    ``git check-ignore``. Empty when ``root`` is not a git work tree or git is
    unavailable, so callers fall back to the static :data:`_NOISE_DIRS` denylist.
    Never raises: ignore-pruning is best-effort and must not break a listing."""
    if not rels:
        return set()
    try:
        proc = subprocess.run(
            ["git", "check-ignore", "--stdin", "-z"],
            input="\0".join(rels) + "\0",
            cwd=root,
            capture_output=True,
            text=True,
            timeout=5,
        )
    except (OSError, subprocess.SubprocessError):
        return set()
    # exit 0 = some ignored, 1 = none, 128 = not a repo: all non-fatal here.
    return {p for p in proc.stdout.split("\0") if p}


__all__ = [
    "ls",
    "tree",
    "cat",
    "read",
    "head",
    "tail",
    "json",
    "diff",
    "edit",
    "img",
    "Code",
    "df_html",
]

# Grayscale palette matching the dashboard, in two themes. Flat and still (no
# gradients/animation): the "sexy" comes from typography, spacing, and a small
# dtype-aware color set, not motion. dtypes are distinguished by lightness, not
# hue (numbers brightest, then strings, then bools, then null). The light values
# mirror the dashboard's own light variables (packages/mcp/site/src/style.css).
_PAL = {
    "panel": "#141416",
    "alt": "#17171a",
    "border": "#242427",
    "head": "#2e2e33",
    "text": "#e6e6e6",
    "muted": "#6a6a70",
    "num": "#e6e6e6",
    "str": "#bcbcc2",
    "bool": "#9a9aa0",
    "null": "#55555b",
    # Background behind the syntax-highlighted Code view (monokai's own).
    "codebg": "#272822",
}
_PAL_LIGHT = {
    "panel": "#ffffff",
    "alt": "#f6f6f8",
    "border": "#e4e4e8",
    "head": "#d4d4da",
    "text": "#1b1b1f",
    "muted": "#80808a",
    "num": "#1b1b1f",
    "str": "#55555c",
    "bool": "#80808a",
    "null": "#a8a8b0",
    "codebg": "#ffffff",
}
_MONO = "ui-monospace,SFMono-Regular,Menlo,monospace"

# Scope class carrying the themeable CSS variables; a top-level output sets it on
# its container so the prefers-color-scheme overrides in `_theme_style` apply.
_THEME_CLASS = "ixv"
# Pygments cssclass for the Code view's token spans (themed by `_code_css`).
_CODE_CLASS = "ixv-hl"


def _c(key: str) -> str:
    """A CSS color reference for ``key``: the themeable ``--ixv-*`` variable with
    the dark value as its fallback. So a fragment renders dark on its own (no
    style block) and flips to light when a `_theme_style` block is present on an
    ancestor (the top-level output emits one; nested fragments inherit it)."""
    return f"var(--ixv-{key},{_PAL[key]})"


def _theme_style() -> str:
    """A ``<style>`` block flipping the ``--ixv-*`` palette to light under
    ``prefers-color-scheme: light``, scoped to ``.ixv``. Emitted once per
    top-level output; the dark values are the inline ``var(...)`` fallbacks, so
    only the light overrides live here."""
    light = ";".join(f"--ixv-{k}:{v}" for k, v in _PAL_LIGHT.items())
    return (
        f"<style>@media(prefers-color-scheme:light)"
        f"{{.{_THEME_CLASS}{{{light}}}}}</style>"
    )


def _code_css() -> str:
    """Embedded two-palette stylesheet for the Code view's highlighted tokens:
    monokai for the dark theme (the default), an xcode-based palette under
    ``prefers-color-scheme: light``, both scoped to ``.ixv-hl``. The light block
    overrides *every* class the dark block colors: monokai paints punctuation
    (the parens, commas, dots) near-white and the light style never restyles it,
    so without the override it would vanish white-on-white. Chrome rules
    (background, line numbers) are dropped so tokens sit on the container box."""
    try:
        from pygments.formatters import HtmlFormatter
    except Exception:
        return ""
    sel = f".{_CODE_CLASS}"

    def token_rules(style_name: str) -> dict[str, str]:
        defs = HtmlFormatter(style=style_name, cssclass=_CODE_CLASS).get_style_defs(sel)
        rules: dict[str, str] = {}
        for line in defs.splitlines():
            stripped = line.strip()
            if not stripped.startswith(f"{sel} ."):
                continue
            if stripped.startswith(f"{sel} .hll"):
                continue
            selector, _, rest = stripped.partition("{")
            rules[selector.strip()] = rest.split("}", 1)[0].strip()
        return rules

    dark = token_rules("monokai")
    light = token_rules("xcode")
    reset = "color: inherit; font-weight: normal; font-style: normal"
    dark_css = "".join(f"{sel_} {{ {decl} }}" for sel_, decl in dark.items())
    light_css = "".join(
        f"{sel_} {{ {light.get(sel_, reset)} }}" for sel_ in {**dark, **light}
    )
    # The `linenos="inline"` line-number spans are chrome, not tokens, so they
    # are not in the palette above. Style them explicitly (theme-aware via the
    # `--ixv-*` vars): a muted color and a gap so they read as a gutter, not as
    # digits fused to the first token, and unselectable so a copy skips them.
    gutter = (
        f"{sel} .linenos {{ color: {_c('muted')}; "
        f"padding-right: 1.25em; user-select: none; -webkit-user-select: none }}"
    )
    return (
        f"<style>{gutter}{dark_css}@media (prefers-color-scheme: light) "
        f"{{{light_css}}}</style>"
    )


def _nested_table(headers, rows, *, key_col=False) -> str:
    """A small bordered inline table for a nested Struct/List value.

    nushell-style: nested data renders as a real boxed sub-table rather than a
    truncated ``str(value)``. ``headers`` is the column labels (None for an
    unlabeled single-column list); each row is a list of pre-rendered cell HTML.
    ``key_col`` left-aligns and mutes the first column (struct field names).
    """
    head = ""
    if headers is not None:
        head = (
            "<thead><tr>"
            + "".join(
                f'<th style="text-align:left;padding:2px 8px;'
                f'border-bottom:1px solid {_c("head")};color:{_c("muted")};'
                f'font-weight:600;white-space:nowrap">{_html.escape(str(h))}</th>'
                for h in headers
            )
            + "</tr></thead>"
        )
    body_rows = ""
    for r in rows:
        tds = ""
        for j, cell in enumerate(r):
            mute = key_col and j == 0
            color = f";color:{_c('muted')}" if mute else ""
            tds += (
                f'<td style="padding:2px 8px;vertical-align:top;'
                f'border-bottom:1px solid {_c("border")};'
                f'font-variant-numeric:tabular-nums{color}">{cell}</td>'
            )
        body_rows += f"<tr>{tds}</tr>"
    return (
        f'<table style="border-collapse:collapse;margin:0;'
        f'border:1px solid {_c("border")};border-radius:4px;'
        f'background:{_c("alt")}">{head}<tbody>{body_rows}</tbody></table>'
    )


_MAX_NESTED_ROWS = 50


def _fmt_nested(value, dtype) -> str | None:
    """Render a Struct/List/Array cell as a nested table; None if not nested."""
    if isinstance(dtype, pl.Struct):
        if value is None:
            return f'<span style="color:{_c("null")};font-style:italic">null</span>'
        fields = {f.name: f.dtype for f in dtype.fields}
        rows = [
            [
                _html.escape(name),
                _fmt_cell(value.get(name), ftype)[0],
            ]
            for name, ftype in fields.items()
        ]
        return _nested_table(None, rows, key_col=True)
    if isinstance(dtype, (pl.List, pl.Array)):
        if value is None:
            return f'<span style="color:{_c("null")};font-style:italic">null</span>'
        inner = dtype.inner
        items = list(value)
        more = ""
        if len(items) > _MAX_NESTED_ROWS:
            extra = len(items) - _MAX_NESTED_ROWS
            items = items[:_MAX_NESTED_ROWS]
            more = (
                f'<div style="color:{_c("muted")};padding:2px 8px;'
                f'font-size:10px">… {extra:,} more</div>'
            )
        if isinstance(inner, pl.Struct):
            # List[Struct] -> a real table: one column per field, one row each.
            cols = [f.name for f in inner.fields]
            ftypes = [f.dtype for f in inner.fields]
            rows = [
                [_fmt_cell((e or {}).get(c), ft)[0] for c, ft in zip(cols, ftypes)]
                for e in items
            ]
            return _nested_table(cols, rows) + more
        # List of scalars (or nested lists) -> single column, one row per element.
        rows = [[_fmt_cell(e, inner)[0]] for e in items]
        return _nested_table(None, rows) + more
    return None


def _fmt_cell(value, dtype) -> tuple[str, str]:
    """Render one cell to (html, align), colored and aligned by dtype."""
    nested = _fmt_nested(value, dtype)
    if nested is not None:
        return nested, "l"
    if value is None:
        return f'<span style="color:{_c("null")};font-style:italic">null</span>', "c"
    if dtype == pl.Boolean:
        return f'<span style="color:{_c("bool")}">{str(value).lower()}</span>', "c"
    try:
        numeric = dtype.is_numeric()
    except Exception:
        numeric = isinstance(value, (int, float))
    if numeric:
        if isinstance(value, int):
            text = f"{value:,}"
        elif isinstance(value, float):
            text = f"{value:,.4g}"
        else:
            text = str(value)
        return f'<span style="color:{_c("num")}">{_html.escape(text)}</span>', "r"
    text = str(value)
    short = text if len(text) <= 60 else text[:57] + "…"
    return (
        f'<span style="color:{_c("str")}" title="{_html.escape(text)}">'
        f"{_html.escape(short)}</span>",
        "l",
    )


def df_html(df: "pl.DataFrame", max_rows: int = 50) -> str:
    """The dashboard's styled HTML for a polars DataFrame (safe wrapper).

    Installed as the global ``pl.DataFrame._repr_html_``; a render failure on some
    exotic frame must never break the human's display, so fall back to polars'
    plain text repr in a ``<pre>`` rather than raising.
    """
    try:
        return _df_html_impl(df, max_rows)
    except Exception:
        return (
            f'{_theme_style()}<pre class="{_THEME_CLASS}" '
            f'style="font-family:{_MONO};font-size:12px;color:{_c("text")};'
            f'background:{_c("panel")};padding:8px;margin:0">'
            f"{_html.escape(str(df))}</pre>"
        )


def _df_html_impl(df: "pl.DataFrame", max_rows: int) -> str:
    """The dashboard's styled HTML for a polars DataFrame.

    The kernel installs this as the global ``polars.DataFrame._repr_html_``, so
    every frame (a ``view`` result, the agent's own, the human's) renders the
    same way and stays a plain ``DataFrame`` that composes with polars. The
    agent's text repr is left to polars, so this never costs the agent tokens.
    """
    cols, dtypes, n = df.columns, df.dtypes, df.height
    head = "".join(
        f'<th style="text-align:left;padding:5px 14px;border-bottom:2px solid '
        f'{_c("head")};white-space:nowrap">'
        f'<div style="color:{_c("text")};font-weight:600">{_html.escape(c)}</div>'
        f'<div style="color:{_c("muted")};font-size:10px">{_html.escape(str(dt))}</div></th>'
        for c, dt in zip(cols, dtypes)
    )
    body = []
    for i, row in enumerate(df.head(max_rows).iter_rows()):
        bg = _c("alt") if i % 2 else _c("panel")
        cells = ""
        for value, dtype in zip(row, dtypes):
            cell, align = _fmt_cell(value, dtype)
            a = {"l": "left", "r": "right", "c": "center"}[align]
            cells += (
                f'<td style="padding:3px 14px;text-align:{a};'
                f'font-variant-numeric:tabular-nums;'
                f'border-bottom:1px solid {_c("border")}">{cell}</td>'
            )
        body.append(f'<tr style="background:{bg}">{cells}</tr>')
    more = (
        f'<div style="color:{_c("muted")};padding:6px 14px;font-size:11px">'
        f"… {n - max_rows:,} more rows</div>"
        if n > max_rows
        else ""
    )
    return (
        f'{_theme_style()}<div class="{_THEME_CLASS}" '
        f'style="display:inline-block;background:{_c("panel")};'
        f'border:1px solid {_c("border")};font-family:{_MONO};font-size:12px;'
        f'color:{_c("text")}">'
        f'<div style="padding:6px 14px;color:{_c("muted")};'
        f'border-bottom:1px solid {_c("border")};letter-spacing:.3px">'
        f"{n:,} rows × {len(cols)} cols</div>"
        f'<table style="border-collapse:collapse;margin:0"><thead><tr>{head}</tr>'
        f"</thead><tbody>{''.join(body)}</tbody></table>{more}</div>"
    )


# --------------------------------------------------------------------------- #
# Code view: a syntax-highlighted file/snippet for the human, raw text for the
# agent. Returned by cat/read/head/tail/json/diff.
# --------------------------------------------------------------------------- #


_EXT_LANG = {
    ".py": "python", ".rs": "rust", ".js": "javascript", ".ts": "typescript",
    ".tsx": "tsx", ".jsx": "jsx", ".nix": "nix", ".sh": "bash", ".bash": "bash",
    ".json": "json", ".toml": "toml", ".yaml": "yaml", ".yml": "yaml",
    ".md": "markdown", ".html": "html", ".css": "css", ".sql": "sql",
    ".c": "c", ".h": "c", ".cpp": "cpp", ".go": "go", ".java": "java",
    ".kt": "kotlin", ".rb": "ruby", ".lua": "lua", ".diff": "diff",
    ".patch": "diff", ".kdl": "text", ".nu": "text",
}


def _highlight(text: str, lang: str | None, start_line: int) -> str:
    from pygments import highlight
    from pygments.formatters import HtmlFormatter
    from pygments.lexers import TextLexer, get_lexer_by_name, guess_lexer

    lexer = None
    if lang:
        try:
            lexer = get_lexer_by_name(lang)
        except Exception:
            lexer = None
    if lexer is None:
        try:
            lexer = guess_lexer(text)
        except Exception:
            lexer = TextLexer()
    # Class-based (not inline) tokens so the palette can flip with the OS theme;
    # the two-palette stylesheet is emitted by `_code_css` onto the Code box.
    formatter = HtmlFormatter(
        style="monokai",
        noclasses=False,
        cssclass=_CODE_CLASS,
        linenos="inline",
        linenostart=start_line,
        nowrap=False,
    )
    return highlight(text, lexer, formatter)


class Code:
    """A syntax-highlighted view of text. ``repr`` is the raw text (what the
    agent reads); ``_repr_html_`` is the highlighted render (what the human sees
    on the dashboard)."""

    def __init__(
        self,
        text: str,
        lang: str | None = None,
        *,
        title: str | None = None,
        start_line: int = 1,
    ) -> None:
        self.text = text
        self.lang = lang
        self.title = title
        self.start_line = start_line

    def __repr__(self) -> str:
        return self.text

    def _repr_html_(self) -> str:
        body = _highlight(self.text, self.lang, self.start_line)
        cap = (
            f'<div style="font-family:{_MONO};font-size:11px;color:{_c("muted")};'
            f'padding:4px 8px">{_html.escape(self.title)}</div>'
            if self.title
            else ""
        )
        return (
            f"{_theme_style()}{_code_css()}"
            f'<div class="{_THEME_CLASS}" style="background:{_c("codebg")};'
            f'border:1px solid {_c("border")};'
            f'font-family:{_MONO};font-size:12px;overflow:auto">{cap}{body}</div>'
        )


def _lang_for(path: pathlib.Path) -> str | None:
    return _EXT_LANG.get(path.suffix.lower())


# --------------------------------------------------------------------------- #
# Tabular helpers -> polars.DataFrame (composable + styled render).
# --------------------------------------------------------------------------- #


def ls(path: str | os.PathLike = ".", *, all: bool = False) -> "pl.DataFrame":
    """A directory listing as a DataFrame (name, kind, size, modified, ignored).

    Dirs sort first, then by name. Hidden entries are skipped unless ``all``.
    ``ignored`` flags the entries the repo's ``.gitignore`` ignores (when ``path``
    is in a git work tree, else always False) -- unlike :func:`tree`, ``ls`` stays
    flat and never drops them, so ``view.ls("node_modules")`` still lists its
    contents; filter with ``.filter(~pl.col("ignored"))`` when you want them gone.
    """
    base = pathlib.Path(path)
    entries = [
        p for p in base.iterdir() if all or not p.name.startswith(".")
    ]
    ignored = (
        set() if all else _git_ignored(base, [p.name for p in entries])
    )
    rows = []
    for p in entries:
        try:
            st = p.stat()
            size = st.st_size if p.is_file() else None
            mtime = datetime.fromtimestamp(st.st_mtime)
        except OSError:
            size, mtime = None, None
        kind = "dir" if p.is_dir() else ("link" if p.is_symlink() else "file")
        rows.append(
            {
                "name": p.name,
                "kind": kind,
                "size": size,
                "modified": mtime,
                "ignored": p.name in ignored,
            }
        )
    df = pl.DataFrame(
        rows,
        schema={
            "name": pl.Utf8,
            "kind": pl.Utf8,
            "size": pl.Int64,
            "modified": pl.Datetime,
            "ignored": pl.Boolean,
        },
    )
    if df.height:
        df = df.sort([(pl.col("kind") != "dir"), "name"])
    return df


def tree(
    path: str | os.PathLike = ".", depth: int = 2, *, all: bool = False
) -> "pl.DataFrame":
    """A recursive listing to ``depth`` as a DataFrame (depth, name, path, kind).

    ``name`` is indented by depth for a tree shape; ``path`` is relative to the
    root so results stay sortable/filterable. Noise is pruned: anything the repo's
    ``.gitignore`` ignores (when ``path`` is in a git work tree), plus a static
    denylist of heavy dirs (``node_modules``, ``target``, ``dist``, ...) so it
    still works outside git. An ignored directory is listed as one collapsed row
    (suffixed ``/…``) but not descended into; an ignored file is dropped. Pass
    ``all=True`` to include hidden + ignored entries and walk everything.
    """
    root = pathlib.Path(path)
    rows = []

    def walk(d: pathlib.Path, level: int) -> None:
        if level > depth:
            return
        try:
            entries = sorted(
                d.iterdir(), key=lambda p: (not p.is_dir(), p.name.lower())
            )
        except OSError:
            return
        ignored = (
            set()
            if all
            else _git_ignored(root, [str(p.relative_to(root)) for p in entries])
        )
        for p in entries:
            if not all and p.name.startswith("."):
                continue
            is_dir = p.is_dir()
            rel = str(p.relative_to(root))
            # Prune noise: anything .gitignore ignores, plus the static denylist
            # of heavy dirs (so it still works outside a git repo). An ignored DIR
            # is shown as one collapsed row (structure stays visible) but not
            # walked; an ignored FILE (build artifact, .env, ...) is dropped.
            noisy = not all and (rel in ignored or (is_dir and p.name in _NOISE_DIRS))
            if noisy and not is_dir:
                continue
            collapsed = noisy and is_dir
            rows.append(
                {
                    "depth": level,
                    "name": ("  " * level) + p.name + ("/\u2026" if collapsed else ""),
                    "path": rel,
                    "kind": "dir" if is_dir else "file",
                }
            )
            if is_dir and not collapsed:
                walk(p, level + 1)

    walk(root, 0)
    return pl.DataFrame(
        rows,
        schema={
            "depth": pl.Int64,
            "name": pl.Utf8,
            "path": pl.Utf8,
            "kind": pl.Utf8,
        },
    )


# --------------------------------------------------------------------------- #
# File helpers -> Code (highlighted view + raw text for the agent).
# --------------------------------------------------------------------------- #


def cat(
    path: str | os.PathLike,
    lines: tuple[int, int] | None = None,
    *,
    lang: str | None = None,
) -> Code:
    """Read a file as a highlighted :class:`Code` view.

    ``lines`` is an inclusive 1-based ``(start, end)`` range to slice.
    """
    p = pathlib.Path(path)
    text = p.read_text(errors="replace")
    start = 1
    if lines is not None:
        a, b = lines
        all_lines = text.splitlines()
        text = "\n".join(all_lines[a - 1 : b])
        start = a
    return Code(text, lang or _lang_for(p), title=str(p), start_line=start)


def read(*args, **kwargs) -> Code:
    """Alias for :func:`cat`."""
    return cat(*args, **kwargs)


def head(path: str | os.PathLike, n: int = 20, *, lang: str | None = None) -> Code:
    """The first ``n`` lines of a file as a :class:`Code` view."""
    p = pathlib.Path(path)
    sliced = p.read_text(errors="replace").splitlines()[:n]
    return Code("\n".join(sliced), lang or _lang_for(p), title=str(p), start_line=1)


def tail(path: str | os.PathLike, n: int = 20, *, lang: str | None = None) -> Code:
    """The last ``n`` lines of a file as a :class:`Code` view."""
    p = pathlib.Path(path)
    all_lines = p.read_text(errors="replace").splitlines()
    start = max(1, len(all_lines) - n + 1)
    return Code(
        "\n".join(all_lines[-n:]), lang or _lang_for(p), title=str(p), start_line=start
    )


def json(obj, *, title: str | None = None) -> Code:
    """Pretty-print JSON as a highlighted :class:`Code` view.

    ``obj`` may be a path to a ``.json`` file, a JSON string, or any
    JSON-serializable object.
    """
    if isinstance(obj, (str, os.PathLike)):
        p = pathlib.Path(obj)
        if p.exists():
            data = _json.loads(p.read_text())
            title = title or str(p)
        else:
            data = _json.loads(str(obj))
    else:
        data = obj
    return Code(_json.dumps(data, indent=2, default=str), "json", title=title)


def diff(
    a, b, *, a_name: str = "a", b_name: str = "b"
) -> Code:
    """A unified diff of two texts or files as a highlighted :class:`Code` view."""

    def _text(x, name):
        if isinstance(x, (str, os.PathLike)) and pathlib.Path(x).exists():
            return pathlib.Path(x).read_text(errors="replace"), str(x)
        return str(x), name

    at, an = _text(a, a_name)
    bt, bn = _text(b, b_name)
    out = difflib.unified_diff(
        at.splitlines(), bt.splitlines(), fromfile=an, tofile=bn, lineterm=""
    )
    return Code("\n".join(out), "diff", title=f"{an} -> {bn}")


def edit(
    path: str | os.PathLike,
    old: str,
    new: str,
    *,
    count: int = 1,
    dry_run: bool = False,
) -> Code:
    """Replace ``old`` with ``new`` in the file at ``path`` and return the change
    as a highlighted unified diff, so an edit is never blind: the human sees
    exactly what moved and you get the same diff as text.

    ``old`` must occur exactly ``count`` times (default 1); pass ``count=N`` for
    an intended N, or ``count=-1`` to replace every occurrence. A miss (pattern
    absent, or a count mismatch) raises ``ValueError`` and writes nothing, so a
    too-broad pattern can never silently rewrite the file. With ``dry_run=True``
    the file is left untouched and only the preview diff is returned.
    """
    p = pathlib.Path(path)
    before = p.read_text()
    found = before.count(old)
    if found == 0:
        raise ValueError(f"edit: pattern not found in {p}")
    if count != -1 and found != count:
        raise ValueError(
            f"edit: pattern found {found}x in {p}, expected {count} "
            f"(pass count={found} to accept, or count=-1 for all)"
        )
    after = before.replace(old, new, -1 if count == -1 else count)
    if not dry_run:
        p.write_text(after)
    label = "edit (preview)" if dry_run else "edit"
    hunks = difflib.unified_diff(
        before.splitlines(),
        after.splitlines(),
        fromfile=f"{p} (before)",
        tofile=f"{p} (after)",
        lineterm="",
    )
    return Code("\n".join(hunks), "diff", title=f"{label} {p}")


def img(path: str | os.PathLike):
    """Open an image file for inline display (returns a ``PIL.Image``)."""
    from PIL import Image

    return Image.open(path)
