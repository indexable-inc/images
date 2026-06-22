#!/usr/bin/env python3
"""Refresh Minecraft loader (Paper / Velocity / Fabric) catalogs.

Reads `packages/minecraft/catalogs/loaders/<loader>/manifest.json`, queries the
upstream metadata service for each version listed there, downloads the
artifact, computes its SHA-256, and writes the per-version lock JSON. The
manifest is the input; the per-version files are generated.

`--check` reuses the same refresh path but compares the result against
disk and exits non-zero on drift, so a weekly CI run surfaces upstream
publication events without writing into the tree.
"""

import argparse
import base64
import hashlib
import json
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

from pydantic import BaseModel, TypeAdapter

JsonObject = dict[str, Any]

USER_AGENT = "indexable-inc/index update-loaders (github.com/indexable-inc/index)"
HEADERS = {"User-Agent": USER_AGENT}

PAPER_FILL_API = "https://fill.papermc.io/v3"
FABRIC_META_API = "https://meta.fabricmc.net/v2"


def http_get(url: str) -> bytes:
    request = urllib.request.Request(url, headers=HEADERS)  # noqa: S310 -- callers only pass https:// URLs (PAPER_FILL_API / FABRIC_META_API constants)
    for attempt in range(3):
        try:
            with urllib.request.urlopen(request) as response:  # noqa: S310 -- same: HTTPS-only, audited at call sites
                if response.status == 429:
                    time.sleep(2**attempt)
                    continue
                body: bytes = response.read()
                return body
        except urllib.error.HTTPError as err:
            if err.code in (429, 502, 503, 504) and attempt < 2:
                time.sleep(2**attempt)
                continue
            raise
    raise RuntimeError(f"rate limited after retries: {url}")


def sri_sha256(data: bytes) -> str:
    return "sha256-" + base64.b64encode(hashlib.sha256(data).digest()).decode()


# The slice of the PaperMC fill v3 builds response this tool depends on. Modeled
# with pydantic so the response is validated at the boundary: if upstream drops
# or renames a field, validation fails with a path-precise error instead of a
# bare KeyError deep in the access chain. Unmodeled fields are ignored.
class _PaperChecksums(BaseModel):
    sha256: str


class _PaperDownload(BaseModel):
    url: str
    checksums: _PaperChecksums


class _PaperBuild(BaseModel):
    id: int
    channel: str | None = None
    downloads: dict[str, _PaperDownload]


_PAPER_BUILDS = TypeAdapter(list[_PaperBuild])


def latest_papermc_build(project: str, version: str, channel: str) -> JsonObject:
    """Resolve the latest build for `(project, version, channel)` on PaperMC.

    Returns `{ "build": int, "url": str, "hash": "sha256-..." }`. The
    `download` URL and SHA-256 reported by the fill v3 API are the durable
    handles; the SHA-256 is converted to SRI before storing.
    """
    builds_url = f"{PAPER_FILL_API}/projects/{project}/versions/{version}/builds"
    builds = _PAPER_BUILDS.validate_json(http_get(builds_url))
    filtered = [build for build in builds if channel == "default" or build.channel == channel]
    if not filtered:
        raise RuntimeError(
            f"{project} {version}: no builds matched channel `{channel}` (got {len(builds)} builds)"
        )
    # PaperMC returns newest first; the latest entry is the canonical build.
    latest = filtered[0]
    download = latest.downloads["server:default"]
    sha256_bytes = bytes.fromhex(download.checksums.sha256)
    sri = "sha256-" + base64.b64encode(sha256_bytes).decode()
    return {
        "build": latest.id,
        "hash": sri,
        "url": download.url,
    }


def fabric_server_lock(
    minecraft_version: str,
    loader_version: str,
    installer_version: str,
) -> JsonObject:
    """Resolve a Fabric server jar by `(mc, loader, installer)` tuple.

    Fabric meta serves a stable jar for the trio, so the URL is fully
    derivable. We still fetch it once to compute the SHA-256 SRI so the
    lock file lets `pkgs.fetchurl` pin the bytes.
    """
    url = (
        f"{FABRIC_META_API}/versions/loader/"
        f"{urllib.parse.quote(minecraft_version)}/"
        f"{urllib.parse.quote(loader_version)}/"
        f"{urllib.parse.quote(installer_version)}/server/jar"
    )
    body = http_get(url)
    return {
        "hash": sri_sha256(body),
        "url": url,
    }


def render_json(value: JsonObject) -> str:
    return json.dumps(value, indent=2, sort_keys=True) + "\n"


def write_or_diff(path: Path, rendered: str, *, check: bool) -> bool:
    """Write `rendered` to `path`, or report drift in `--check` mode.

    Returns True when the file would change.
    """
    current = path.read_text() if path.exists() else None
    if current == rendered:
        print(f"  {path}: up to date", file=sys.stderr)
        return False

    if check:
        print(f"  {path}: drift detected", file=sys.stderr)
        if current is None:
            print(f"    (missing on disk)", file=sys.stderr)
        return True

    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(rendered)
    print(f"  {path}: wrote", file=sys.stderr)
    return True


def refresh_papermc(root: Path, only_version: str | None, *, check: bool) -> bool:
    """Refresh Paper or Velocity locks. Returns True if anything would change."""
    manifest_path = root / "manifest.json"
    manifest = json.loads(manifest_path.read_text())
    project = manifest["project"]
    channel = manifest.get("releaseChannel", "default")
    versions = manifest["versions"]

    drift = False
    for version in versions:
        if only_version and version != only_version:
            continue
        print(f"{root.name}/{version}: querying fill.papermc.io", file=sys.stderr)
        lock = latest_papermc_build(project, version, channel)
        rendered = render_json(lock)
        drift |= write_or_diff(root / f"{version}.json", rendered, check=check)
    return drift


def refresh_fabric(root: Path, only_version: str | None, *, check: bool) -> bool:
    manifest_path = root / "manifest.json"
    manifest = json.loads(manifest_path.read_text())
    loader_version = manifest["loaderVersion"]
    installer_version = manifest["installerVersion"]
    versions = manifest["versions"]

    drift = False
    for version in versions:
        if only_version and version != only_version:
            continue
        print(f"fabric/{version}: fetching meta.fabricmc.net", file=sys.stderr)
        lock = fabric_server_lock(version, loader_version, installer_version)
        rendered = render_json(lock)
        drift |= write_or_diff(root / f"{version}.json", rendered, check=check)
    return drift


REFRESHERS = {
    "paper": refresh_papermc,
    "velocity": refresh_papermc,
    "fabric": refresh_fabric,
}


def loader_root(loaders_root: Path, loader: str) -> Path:
    return loaders_root / loader


def default_loaders_root() -> Path:
    cwd_root = Path.cwd() / "packages/minecraft/catalogs/loaders"
    if cwd_root.exists():
        return cwd_root
    return Path(__file__).resolve().parent.parent / "packages/minecraft/catalogs/loaders"


def main() -> None:
    parser = argparse.ArgumentParser(description="Refresh Minecraft loader manifests")
    parser.add_argument(
        "--loaders-root",
        type=Path,
        help="Path to packages/minecraft/catalogs/loaders/",
    )
    parser.add_argument(
        "--loader",
        choices=sorted(REFRESHERS.keys()),
        action="append",
        help="Refresh only this loader. May be repeated. Defaults to all.",
    )
    parser.add_argument(
        "--version",
        dest="only_version",
        help="Refresh only this game/proxy version. Useful for targeted bumps.",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="Verify on-disk locks match upstream without writing. Exits 1 on drift.",
    )
    args = parser.parse_args()

    loaders_root = args.loaders_root or default_loaders_root()
    if not loaders_root.is_dir():
        raise SystemExit(f"loaders root does not exist: {loaders_root}")

    selected = args.loader or sorted(REFRESHERS.keys())
    drift = False
    for loader in selected:
        root = loader_root(loaders_root, loader)
        if not (root / "manifest.json").exists():
            raise SystemExit(f"missing manifest: {root / 'manifest.json'}")
        refresher = REFRESHERS[loader]
        drift |= refresher(root, args.only_version, check=args.check)

    if args.check and drift:
        sys.exit(1)


if __name__ == "__main__":
    main()
