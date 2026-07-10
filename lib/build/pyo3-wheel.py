#!/usr/bin/env python3
"""Package a pre-built PyO3 cdylib and Python sources into a PEP 427 wheel."""

from __future__ import annotations

import argparse
import base64
import hashlib
from pathlib import Path
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
    summary: str,
    requires_dist: list[str],
    cdylib: Path,
    python_src: Path,
    version: str,
    platform_tag: str,
    out: Path,
) -> Path:
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

    requirements = "".join(f"Requires-Dist: {requirement}\n" for requirement in requires_dist)
    files[f"{dist_info}/METADATA"] = (
        "Metadata-Version: 2.4\n"
        f"Name: {dist_name}\n"
        f"Version: {version}\n"
        f"Summary: {summary}\n"
        "Author: indexable\n"
        "Requires-Python: >=3.11\n"
        f"{requirements}"
    ).encode()
    files[f"{dist_info}/WHEEL"] = (
        "Wheel-Version: 1.0\n"
        "Generator: pyo3-wheel\n"
        "Root-Is-Purelib: false\n"
        f"Tag: {tag}\n"
    ).encode()

    records = [record_line(name, data) for name, data in files.items()]
    records.append(f"{dist_info}/RECORD,,")
    files[f"{dist_info}/RECORD"] = "\n".join(records).encode() + b"\n"

    out.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(wheel_path, "w", zipfile.ZIP_DEFLATED) as wheel:
        for name in sorted(files):
            wheel.writestr(name, files[name])

    return wheel_path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--package", required=True)
    parser.add_argument("--dist-name", required=True)
    parser.add_argument("--so-name", required=True)
    parser.add_argument("--summary", required=True)
    parser.add_argument("--requires-dist", action="append", default=[])
    parser.add_argument("--cdylib", type=Path, required=True)
    parser.add_argument("--python-src", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--platform-tag", required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()

    print(
        build_wheel(
            package=args.package,
            dist_name=args.dist_name,
            so_name=args.so_name,
            summary=args.summary,
            requires_dist=args.requires_dist,
            cdylib=args.cdylib,
            python_src=args.python_src,
            version=args.version,
            platform_tag=args.platform_tag,
            out=args.out,
        )
    )


if __name__ == "__main__":
    main()
