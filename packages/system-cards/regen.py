"""Sync the built system-cards corpus into the working tree.

The corpus is a pure derivation (pinned PDFs converted by pymupdf4llm); this
app copies it over packages/system-cards/cards so the committed markdown is
exactly the build product.
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
from pathlib import Path


def repo_root() -> Path:
    out = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        check=True,
        capture_output=True,
        text=True,
    )
    return Path(out.stdout.strip())


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, required=True)
    options = parser.parse_args()

    target = repo_root() / "packages" / "system-cards" / "cards"
    if target.exists():
        shutil.rmtree(target)
    shutil.copytree(options.corpus, target)
    # The copy inherits read-only store modes; reopen so the next sync can replace it.
    for path in target.rglob("*"):
        path.chmod(0o755 if path.is_dir() else 0o644)
    target.chmod(0o755)
    count = sum(1 for _ in target.rglob("*.md"))
    print(f"synced {count} cards into {target}")


if __name__ == "__main__":
    main()
