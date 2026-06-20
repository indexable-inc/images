"""Resolve the system prompt under test, rendering ``system-prompt.nix`` if asked.

Resolution order:

1. ``--system-prompt-file <path>``: a prompt text file (used to test a candidate
   edit before it is committed).
2. ``--system-prompt-nix <path>``: render that ``.nix`` to text with ``nix eval``.
3. ``SYSTEM_PROMPT_EVAL_PROMPT_FILE``: the prompt the Nix wrapper baked in.
4. checkout fallback: render ``packages/agent/system-prompt.nix`` next to the
   datasets.

A short content hash of the resolved prompt is recorded in the report so a score
is always attributable to an exact prompt.
"""

from __future__ import annotations

import hashlib
import subprocess
import tempfile
from pathlib import Path

from .paths import baked_prompt_file, repo_system_prompt_nix


class PromptError(RuntimeError):
    """The prompt under test could not be resolved or rendered."""


def render_nix(nix_path: Path) -> str:
    """Render a ``system-prompt.nix`` to plain text via ``nix eval --raw``."""
    expr = f"import {nix_path} {{ lib = (import <nixpkgs> {{}}).lib; }}"
    try:
        proc = subprocess.run(
            ["nix", "eval", "--raw", "--impure", "--expr", expr],
            capture_output=True,
            text=True,
            check=False,
        )
    except FileNotFoundError as exc:
        raise PromptError("`nix` not found on PATH; pass --system-prompt-file") from exc
    if proc.returncode != 0:
        raise PromptError(f"nix eval failed: {proc.stderr.strip()[:400]}")
    return proc.stdout


def resolve_prompt(
    system_prompt_file: Path | None, system_prompt_nix: Path | None
) -> tuple[Path, str]:
    """Resolve to (prompt_file_on_disk, sha256_prefix).

    Always returns a real file path that can be passed to ``--system-prompt-file``;
    a rendered prompt is written to a temp file that lives for the process.
    """
    if system_prompt_file is not None:
        text = system_prompt_file.read_text(encoding="utf-8")
        return system_prompt_file, _sha(text)
    if system_prompt_nix is not None:
        return _materialize(render_nix(system_prompt_nix))
    baked = baked_prompt_file()
    if baked is not None and baked.exists():
        return baked, _sha(baked.read_text(encoding="utf-8"))
    fallback = repo_system_prompt_nix()
    if fallback.exists():
        return _materialize(render_nix(fallback))
    raise PromptError(
        "no prompt to test: pass --system-prompt-file or --system-prompt-nix"
    )


def _materialize(text: str) -> tuple[Path, str]:
    # A rendered prompt must outlive this function (it is passed to a subprocess),
    # so write it into a temp dir rather than a self-deleting NamedTemporaryFile.
    path = Path(tempfile.mkdtemp(prefix="house-prompt-")) / "system-prompt.txt"
    path.write_text(text, encoding="utf-8")
    return path, _sha(text)


def _sha(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()[:12]
