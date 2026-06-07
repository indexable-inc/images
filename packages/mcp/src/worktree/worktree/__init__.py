"""Git worktrees as the unit of isolated work for the ix-mcp kernel.

Bundled like ``view``/``sh``/``nix`` so every session can ``import worktree`` with
no setup. The point: do risky or parallel work on a throwaway branch in its own
checked-out tree, so the main working copy is never disturbed and two lines of
work never stomp each other. Built on async ``git worktree`` (so checking out a
big repo never blocks the one shared event loop), it folds in the gotchas an
agent keeps hitting:

* a flake builds the files git *knows about*, so a brand-new file you have not
  staged is invisible to ``nix build`` -- it silently builds the old tree.
  :meth:`Worktree.build` runs ``git add -A`` first so new files are seen, then
  builds the worktree.
* an edit by absolute path lands in whatever tree owns that path, so operate on a
  worktree *through it*: ``wt / "sub/file"`` is the path inside it (use it with
  ``view.edit``/``view.cat``), and :meth:`Worktree.sh` / :meth:`Worktree.build` /
  :meth:`Worktree.commit` thread the right ``cwd`` for you.

    import worktree
    wt = await worktree.add("my-fix")               # new branch + tree off HEAD
    view.edit(wt / "packages/mcp/.../x.py", a, b)   # edit inside the worktree
    await wt.build(".#mcp")                          # stages, then nix-builds it
    await wt.commit("mcp: my fix")                   # when it is good
    await wt.remove()                                # tree gone (branch kept)

:func:`list` returns a polars DataFrame (sort/filter it; it renders as the
dashboard's styled table for free); the mutating calls are async.
"""

from __future__ import annotations

import dataclasses
import html as _html
import os
import pathlib
import subprocess
import tempfile

import polars as pl

__all__ = ["list", "add", "remove", "prune", "Worktree"]

# `list` below intentionally shadows the builtin -- it is the natural name for the
# listing, and nothing in this module needs the builtin by that name.
_MONO = "ui-monospace,SFMono-Regular,Menlo,monospace"


def _run(repo: str | os.PathLike, *args: str) -> subprocess.CompletedProcess:
    """Run a ``git`` command in ``repo`` and return the completed process.

    Synchronous and used only for the fast, metadata-only reads (``rev-parse``,
    ``worktree list``); the mutating operations go through the async, non-blocking
    bundled ``sh`` instead so a checkout never freezes the kernel's event loop.
    """
    return subprocess.run(
        ["git", "-C", str(repo), *args],
        capture_output=True,
        text=True,
    )


def _toplevel(path: str | os.PathLike) -> pathlib.Path:
    """The main work tree root for the repo containing ``path``."""
    proc = _run(path, "rev-parse", "--show-toplevel")
    if proc.returncode != 0:
        raise ValueError(f"not inside a git repository: {path}")
    return pathlib.Path(proc.stdout.strip())


def _sanitize(branch: str) -> str:
    """A single safe path component for ``branch`` (``codex/foo`` -> ``codex-foo``)."""
    return branch.strip("/").replace("/", "-") or "worktree"


def _default_path(repo: pathlib.Path, branch: str) -> pathlib.Path:
    """Where a worktree lands when no ``path`` is given: a per-repo temp dir,
    keeping linked trees out of the repo (so they never show up in its status or
    get caught by its ``.gitignore``)."""
    base = pathlib.Path(tempfile.gettempdir()) / "ix-worktrees" / repo.name
    return base / _sanitize(branch)


def _local_branch_exists(repo: str | os.PathLike, branch: str) -> bool:
    return (
        _run(repo, "show-ref", "--verify", "--quiet", f"refs/heads/{branch}").returncode
        == 0
    )


@dataclasses.dataclass
class Worktree:
    """One linked work tree: a branch checked out at its own ``path``.

    Returned by :func:`add` and the rows of :func:`list`. It is also an
    ``os.PathLike`` (``__fspath__`` is its path) and ``wt / "sub"`` joins onto it,
    so it drops straight into ``view.cat`` / ``view.edit`` / ``pathlib``.
    """

    path: pathlib.Path
    branch: str | None
    head: str
    repo: pathlib.Path
    locked: bool = False
    prunable: bool = False

    def __fspath__(self) -> str:
        return str(self.path)

    def __truediv__(self, other: str | os.PathLike) -> pathlib.Path:
        return self.path / other

    async def sh(self, cmd, **kwargs):
        """Run a shell command in this worktree (bundled ``sh``, ``cwd`` threaded)."""
        import sh as _sh

        return await _sh(cmd, cwd=str(self.path), **kwargs)

    async def commit(self, message: str, *, all: bool = True):
        """Commit this worktree. With ``all`` (the default) stage everything first
        (``git add -A``), so new files are included; returns the ``git commit``
        :class:`sh.Output`."""
        import sh as _sh

        if all:
            await _sh(["git", "-C", str(self.path), "add", "-A"], check=True)
        return await _sh(["git", "-C", str(self.path), "commit", "-m", message])

    async def build(self, attr: str, *flags: str, add: bool = True, **kwargs):
        """``nix build`` this worktree's flake (bundled ``nix``, ``cwd`` threaded).

        With ``add`` (the default) ``git add -A`` runs first, because a flake only
        sees files git tracks -- an unstaged new file would be invisible and the
        build would silently use the old tree. Returns the :class:`nix.NixLog`.
        """
        import nix as _nix

        if add:
            import sh as _sh

            await _sh(["git", "-C", str(self.path), "add", "-A"], check=True)
        return await _nix.build(attr, *flags, cwd=str(self.path), **kwargs)

    async def remove(self, *, force: bool = False):
        """Remove this linked work tree (the branch is kept). ``force`` discards
        uncommitted changes in it; returns the ``git worktree remove``
        :class:`sh.Output`."""
        return await remove(self.path, repo=self.repo, force=force)

    def __repr__(self) -> str:
        ref = self.branch or f"({self.head[:8]} detached)"
        suffix = " [locked]" if self.locked else ""
        return f"Worktree({ref} @ {self.path}{suffix})"

    def _repr_html_(self) -> str:
        ref = _html.escape(self.branch or f"{self.head[:8]} (detached)")
        tags = ""
        if self.locked:
            tags += '<span style="color:#fc618d"> locked</span>'
        if self.prunable:
            tags += '<span style="color:#fce566"> prunable</span>'
        return (
            f'<div style="display:inline-block;background:#141416;'
            f"border:1px solid #242427;border-radius:6px;padding:8px 12px;"
            f'font-family:{_MONO};font-size:12px;color:#e6e6e6">'
            f'<div style="color:#7bd88f;font-weight:600">{ref}{tags}</div>'
            f'<div style="color:#6a6a70">{_html.escape(str(self.path))}'
            f" · {self.head[:8]}</div></div>"
        )


def _parse_porcelain(text: str, repo: pathlib.Path) -> "pl.DataFrame":
    """Parse ``git worktree list --porcelain`` (blank-line-separated stanzas)."""
    rows: list = []
    cur: dict = {}

    def flush() -> None:
        if cur:
            rows.append(dict(cur))
            cur.clear()

    for line in text.splitlines():
        if not line:
            flush()
            continue
        key, _, val = line.partition(" ")
        if key == "worktree":
            flush()
            cur["path"] = val
        elif key == "HEAD":
            cur["head"] = val
        elif key == "branch":
            cur["branch"] = val.removeprefix("refs/heads/")
        elif key in ("bare", "detached", "locked", "prunable"):
            cur[key] = True
    flush()

    return pl.DataFrame(
        [
            {
                "path": r.get("path", ""),
                "branch": r.get("branch"),
                "head": (r.get("head") or "")[:8],
                "locked": bool(r.get("locked")),
                "prunable": bool(r.get("prunable")),
                "current": pathlib.Path(r.get("path", "")) == repo,
            }
            for r in rows
        ],
        schema={
            "path": pl.Utf8,
            "branch": pl.Utf8,
            "head": pl.Utf8,
            "locked": pl.Boolean,
            "prunable": pl.Boolean,
            "current": pl.Boolean,
        },
    )


def list(repo: str | os.PathLike = ".") -> "pl.DataFrame":  # noqa: A001
    """Every linked work tree of ``repo`` as a DataFrame (path, branch, head,
    locked, prunable, current).

    ``current`` marks the main work tree. Fast (metadata only), so it is sync and
    returns a plain polars frame you can ``.filter`` / ``.sort``.
    """
    top = _toplevel(repo)
    proc = _run(repo, "worktree", "list", "--porcelain")
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.strip() or "git worktree list failed")
    return _parse_porcelain(proc.stdout, top)


async def add(
    branch: str,
    *,
    base: str | None = None,
    path: str | os.PathLike | None = None,
    repo: str | os.PathLike = ".",
    force: bool = False,
) -> Worktree:
    """Create a linked work tree for ``branch`` and return its :class:`Worktree`.

    A new local branch is created off ``base`` (default: the repo's current HEAD);
    if ``branch`` already exists it is checked out instead. ``path`` defaults to a
    per-repo temp dir outside the repo so the tree never pollutes its status or
    ``.gitignore``. ``force`` allows reusing a path or a branch already checked
    out elsewhere. Raises :class:`sh.ShellError` (carrying git's output) on
    failure, so a clash never half-creates a tree.
    """
    import sh as _sh

    top = _toplevel(repo)
    dest = pathlib.Path(path) if path is not None else _default_path(top, branch)
    argv = ["git", "-C", str(top), "worktree", "add"]
    if force:
        argv.append("--force")
    if _local_branch_exists(top, branch):
        # Existing branch: check it out into the new tree (no -b, no base).
        argv += [str(dest), branch]
    else:
        argv += ["-b", branch, str(dest)]
        if base is not None:
            argv.append(base)
    await _sh(argv, check=True)
    head = _run(dest, "rev-parse", "HEAD").stdout.strip()
    return Worktree(path=dest.resolve(), branch=branch, head=head, repo=top)


async def remove(
    target: str | os.PathLike,
    *,
    repo: str | os.PathLike = ".",
    force: bool = False,
):
    """Remove the linked work tree at ``target`` (a path), keeping its branch.

    ``force`` discards uncommitted changes in the tree. Returns the ``git worktree
    remove`` :class:`sh.Output`.
    """
    import sh as _sh

    top = _toplevel(repo)
    argv = ["git", "-C", str(top), "worktree", "remove"]
    if force:
        argv.append("--force")
    argv.append(str(target))
    return await _sh(argv)


async def prune(repo: str | os.PathLike = "."):
    """Prune administrative metadata for work trees whose directory is gone.

    Returns the ``git worktree prune`` :class:`sh.Output`.
    """
    import sh as _sh

    top = _toplevel(repo)
    return await _sh(["git", "-C", str(top), "worktree", "prune", "-v"])
