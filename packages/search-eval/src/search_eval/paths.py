"""Resolve the on-disk locations of the corpus and datasets.

The corpus and eval sets ship as plain files at the package root, not as
importable resources, because `search` needs a real directory to index. Two
resolution paths:

- ``SEARCH_EVAL_DATA_DIR``: set by the Nix wrapper to the installed data tree.
- source layout: when running from a checkout, derive the package root from this
  file's location (``.../packages/search-eval``).
"""

from __future__ import annotations

import os
from pathlib import Path


def data_root() -> Path:
    """Directory holding ``corpus/`` and ``datasets/``."""
    env = os.environ.get("SEARCH_EVAL_DATA_DIR")
    if env:
        return Path(env)
    # src/search_eval/paths.py -> packages/search-eval
    return Path(__file__).resolve().parents[2]


def corpus_dir() -> Path:
    """The fixture corpus `search` indexes and searches over."""
    return data_root() / "corpus"


def dataset_path(name: str) -> Path:
    """A packaged dataset file under ``datasets/``."""
    return data_root() / "datasets" / name
