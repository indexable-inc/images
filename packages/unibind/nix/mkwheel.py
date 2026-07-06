#!/usr/bin/env python3
"""Package a pre-built PyO3 cdylib plus its Python package tree into a PEP 427 wheel.

Nix (`unibind.lib.build`, packages/unibind/nix) builds the crate's cdylib
through the shared cargo-unit workspace graph and calls this to assemble the
wheel, so there is no maturin / PEP 517 backend in the loop. Generalized from
packages/search/search-py/wheel/mkwheel.py: the package, distribution, and
native-module names arrive as arguments, and the Python sources are whatever
files sit under `--python-src/<package>` (the merged generated + hand-written
site tree). The extension is abi3 (`pyo3/abi3-py311`), hence the `cp311-abi3`
tag: one wheel loads on CPython 3.11+.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import pathlib
import zipfile


def sha256_b64(data: bytes) -> str:
    digest = hashlib.sha256(data).digest()
    return base64.urlsafe_b64encode(digest).rstrip(b"=").decode("ascii")


def record_line(name: str, data: bytes) -> str:
    return f"{name},sha256={sha256_b64(data)},{len(data)}"


def build_wheel(
    *,
    package: str,
    dist_name: str,
    so_name: str,
    cdylib: pathlib.Path,
    python_src: pathlib.Path,
    version: str,
    platform_tag: str,
    out: pathlib.Path,
) -> pathlib.Path:
    # PEP 427 escaping: `-` becomes `_` in the wheel filename and dist-info.
    dist = dist_name.replace("-", "_")
    tag = f"cp311-abi3-{platform_tag}"
    dist_info = f"{dist}-{version}.dist-info"
    wheel_path = out / f"{dist}-{version}-{tag}.whl"

    files: dict[str, bytes] = {}
    package_dir = python_src / package
    for path in sorted(package_dir.rglob("*")):
        if path.is_file():
            files[f"{package}/{path.relative_to(package_dir)}"] = path.read_bytes()
    files[f"{package}/{so_name}"] = cdylib.read_bytes()

    files[f"{dist_info}/METADATA"] = (
        "Metadata-Version: 2.4\n"
        f"Name: {dist_name}\n"
        f"Version: {version}\n"
        f"Summary: PyO3 bindings imported as `{package}`.\n"
        "Author: indexable\n"
        "Requires-Python: >=3.11\n"
    ).encode()
    files[f"{dist_info}/WHEEL"] = (
        "Wheel-Version: 1.0\n"
        "Generator: unibind-mkwheel\n"
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
    p.add_argument("--package", required=True)
    p.add_argument("--dist-name", required=True)
    p.add_argument("--so-name", required=True)
    p.add_argument("--cdylib", type=pathlib.Path, required=True)
    p.add_argument("--python-src", type=pathlib.Path, required=True)
    p.add_argument("--version", required=True)
    p.add_argument("--platform-tag", required=True)
    p.add_argument("--out", type=pathlib.Path, required=True)
    args = p.parse_args()

    print(
        build_wheel(
            package=args.package,
            dist_name=args.dist_name,
            so_name=args.so_name,
            cdylib=args.cdylib,
            python_src=args.python_src,
            version=args.version,
            platform_tag=args.platform_tag,
            out=args.out,
        )
    )


if __name__ == "__main__":
    main()
