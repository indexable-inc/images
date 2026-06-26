"""Strip uv's non-reproducible build-provenance stamp from an installed venv.

`uv build` writes a `uv_cache.json` into the local project's dist-info carrying
a wall-clock timestamp, so the venv (and its NAR) differs per build and the
stamp's hash flips the dist-info RECORD line. The file is build-cache metadata
with no runtime role; removing it and its RECORD entry makes the install
bit-identical. Run as: python strip-uv-cache-stamp.py <venv-dir>.
"""

import pathlib
import sys


def main() -> None:
    venv = pathlib.Path(sys.argv[1])
    for stamp in venv.rglob("*.dist-info/uv_cache.json"):
        record = stamp.parent / "RECORD"
        if record.exists():
            kept = [
                line
                for line in record.read_text().splitlines()
                if "/uv_cache.json," not in line
            ]
            record.write_text("".join(f"{line}\n" for line in kept))
        stamp.unlink()


if __name__ == "__main__":
    main()
