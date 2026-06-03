#!/usr/bin/env python3
"""Package a pre-built PyO3 cdylib plus the Python source into a PEP 427 wheel.

Nix builds the `polars-mixedbread` cdylib through cargo-unit and calls this to
assemble the wheel, so there is no maturin / PEP 517 backend in the loop. The
extension is abi3 (`pyo3/abi3-py311`), hence the `cp311-abi3` tag: one wheel
loads on CPython 3.11+.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import pathlib
import zipfile

# Import package vs. PyPI distribution: `import polars_mixedbread`, and the wheel
# and dist-info carry the same (already-normalized) distribution name.
PKG = "polars_mixedbread"
DIST = "polars_mixedbread"
DIST_NAME = "polars-mixedbread"
SO_NAME = "_polars_mixedbread.abi3.so"
# Files copied verbatim from the Python source tree into the wheel.
SOURCE_FILES = ["__init__.py", "_pushdown.py", "_polars_mixedbread.pyi", "py.typed"]


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
        "Summary: Polars IO source backed by Mixedbread store search. "
        "Imported as `polars_mixedbread`.\n"
        "Author: indexable\n"
        "Requires-Python: >=3.11\n"
        "Requires-Dist: polars\n"
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
    # Deterministic order *and* a fixed timestamp so the wheel is byte-for-byte
    # reproducible: `writestr` with a bare name would stamp each entry with the
    # current local time. 1980-01-01 is the zip epoch (the earliest a DOS-time
    # field can encode).
    with zipfile.ZipFile(wheel_path, "w", zipfile.ZIP_DEFLATED) as zf:
        for name in sorted(files):
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            zf.writestr(info, files[name])

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
