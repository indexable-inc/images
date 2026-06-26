"""Strip uv's non-reproducible build-provenance stamps from an installed venv.

uv writes a `uv_cache.json` into every installed package's dist-info carrying a
wall-clock timestamp, so the venv (and its NAR) differs per build and each
stamp's hash flips its dist-info RECORD line. The files are build-cache metadata
with no runtime role; removing them and their RECORD entries makes the install
bit-identical. Run as: python strip-uv-cache-stamp.py <venv-dir>.
"""

import pathlib
import sys


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: strip-uv-cache-stamp.py <venv-dir>")
    venv = pathlib.Path(sys.argv[1])
    for stamp in venv.rglob("*.dist-info/uv_cache.json"):
        record = stamp.parent / "RECORD"
        if record.exists():
            # RECORD lines are `path,sha256=...,size`, path relative to
            # site-packages. Drop the line whose path field is exactly this
            # stamp (not a substring match) so a same-named data file elsewhere
            # stays listed. A stamp with no RECORD entry is left for the
            # unlink below; removing the orphan file is still reproducible.
            target = f"{stamp.parent.name}/uv_cache.json"
            kept = [
                line
                for line in record.read_text(encoding="utf-8").splitlines()
                if line.split(",", 1)[0] != target
            ]
            record.write_text(
                "".join(f"{line}\n" for line in kept), encoding="utf-8"
            )
        stamp.unlink()


if __name__ == "__main__":
    main()
