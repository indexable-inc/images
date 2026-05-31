#!/usr/bin/env python3
"""Package a pre-built PyO3 cdylib plus the Python source into a PEP 427 wheel.

Nix builds the `search-py` cdylib through cargo-unit and calls this to
assemble the `ix-search` wheel, so there is no maturin / PEP 517
backend in the loop. The extension is abi3 (`pyo3/abi3-py311`), hence the
`cp311-abi3` tag: one wheel loads on CPython 3.11+.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import pathlib
import zipfile

# Import package vs. PyPI distribution: `import search`, but the wheel
# and dist-info carry the distribution name `ix-search` (normalized to
# `ix_search`).
PKG = "search"
DIST = "ix_search"
DIST_NAME = "ix-search"
SO_NAME = "_search.abi3.so"
# Files copied verbatim from the Python source tree into the wheel.
SOURCE_FILES = ["__init__.py", "_search.pyi", "py.typed"]


def sha256_b64(data: bytes) -> str:
    digest = hashlib.sha256(data).digest()
    return base64.urlsafe_b64encode(digest).rstrip(b"=").decode("ascii")


def record_line(name: str, data: bytes) -> str:
    return f"{name},sha256={sha256_b64(data)},{len(data)}"


def build_wheel(
    *,
    cdylib: pathlib.Path,
    python_src: pathlib.Path,
    version: str,
    platform_tag: str,
    out: pathlib.Path,
) -> pathlib.Path:
    tag = f"cp311-abi3-{platform_tag}"
    dist_info = f"{DIST}-{version}.dist-info"
    wheel_path = out / f"{DIST}-{version}-{tag}.whl"

    files: dict[str, bytes] = {}
    for name in SOURCE_FILES:
        files[f"{PKG}/{name}"] = (python_src / PKG / name).read_bytes()
    files[f"{PKG}/{SO_NAME}"] = cdylib.read_bytes()

    files[f"{dist_info}/METADATA"] = (
        "Metadata-Version: 2.4\n"
        f"Name: {DIST_NAME}\n"
        f"Version: {version}\n"
        "Summary: Python bindings for content-addressed semantic code search. "
        "Imported as `search`.\n"
        "Author: indexable\n"
        "Requires-Python: >=3.11\n"
    ).encode()
    files[f"{dist_info}/WHEEL"] = (
        "Wheel-Version: 1.0\n"
        "Generator: mkwheel\n"
        "Root-Is-Purelib: false\n"
        f"Tag: {tag}\n"
    ).encode()

    records = [record_line(name, data) for name, data in files.items()]
    records.append(f"{dist_info}/RECORD,,")
    files[f"{dist_info}/RECORD"] = "\n".join(records).encode() + b"\n"

    out.mkdir(parents=True, exist_ok=True)
    # Deterministic order so the wheel hash is stable across builds.
    with zipfile.ZipFile(wheel_path, "w", zipfile.ZIP_DEFLATED) as zf:
        for name in sorted(files):
            zf.writestr(name, files[name])

    return wheel_path


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--cdylib", type=pathlib.Path, required=True)
    p.add_argument("--python-src", type=pathlib.Path, required=True)
    p.add_argument("--version", default="0.1.0")
    p.add_argument("--platform-tag", default="manylinux_2_34_x86_64")
    p.add_argument("--out", type=pathlib.Path, required=True)
    args = p.parse_args()

    print(
        build_wheel(
            cdylib=args.cdylib,
            python_src=args.python_src,
            version=args.version,
            platform_tag=args.platform_tag,
            out=args.out,
        )
    )


if __name__ == "__main__":
    main()
