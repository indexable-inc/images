"""Resolve on-disk locations of the datasets and the prompt under test.

Two resolution paths, mirroring ``search-eval``:

- ``SYSTEM_PROMPT_EVAL_DATA_DIR``: set by the Nix wrapper to the installed data
  tree (holds ``datasets/``).
- source layout: when running from a checkout, derive the package root from this
  file's location (``.../packages/agent/system-prompt-eval``).

``SYSTEM_PROMPT_EVAL_PROMPT_FILE`` is set by the Nix wrapper to the rendered
house prompt, so ``nix run .#system-prompt-eval`` tests exactly the committed
prompt without re-rendering. A ``--system-prompt-file`` flag overrides it.
"""

from __future__ import annotations

import os
from pathlib import Path


def data_root() -> Path:
    """Directory holding ``datasets/``."""
    env = os.environ.get("SYSTEM_PROMPT_EVAL_DATA_DIR")
    if env:
        return Path(env)
    # src/system_prompt_eval/paths.py -> packages/agent/system-prompt-eval
    return Path(__file__).resolve().parents[2]


def dataset_path(name: str) -> Path:
    """A packaged dataset file under ``datasets/``."""
    return data_root() / "datasets" / name


def baked_prompt_file() -> Path | None:
    """The rendered prompt the Nix wrapper baked in, if present."""
    env = os.environ.get("SYSTEM_PROMPT_EVAL_PROMPT_FILE")
    return Path(env) if env else None


def repo_system_prompt_nix() -> Path:
    """The house prompt source in a checkout (``packages/agent/system-prompt.nix``)."""
    return data_root().parent / "system-prompt.nix"
