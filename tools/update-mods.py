#!/usr/bin/env python3
"""Query Modrinth and generate per-version mod URL+hash catalogs."""

import argparse
import base64
import json
import sys
import time
import urllib.request
from pathlib import Path

API = "https://api.modrinth.com/v2"
HEADERS = {"User-Agent": "ix-images/update-mods (github.com/indexable-inc/images)"}

_project_cache: dict[str, dict] = {}


def api_get(path: str, params: dict | None = None):
    url = f"{API}{path}"
    if params:
        from urllib.parse import urlencode
        url += "?" + urlencode(params)
    req = urllib.request.Request(url, headers=HEADERS)
    for attempt in range(3):
        try:
            with urllib.request.urlopen(req) as resp:
                if resp.status == 429:
                    time.sleep(2 ** attempt)
                    continue
                return json.loads(resp.read())
        except urllib.error.HTTPError as e:
            if e.code == 429 and attempt < 2:
                time.sleep(2 ** attempt)
                continue
            raise
    raise RuntimeError(f"rate limited after retries: {url}")


def get_project(id_or_slug: str) -> dict:
    if id_or_slug not in _project_cache:
        _project_cache[id_or_slug] = api_get(f"/project/{id_or_slug}")
        _project_cache[_project_cache[id_or_slug]["id"]] = _project_cache[id_or_slug]
    return _project_cache[id_or_slug]


def get_versions(project_id: str, game_versions: list[str], loader: str) -> list[dict]:
    return api_get(f"/project/{project_id}/version", {
        "game_versions": json.dumps(game_versions),
        "loaders": json.dumps([loader]),
    })


def pick_version(versions: list[dict]) -> dict | None:
    if not versions:
        return None
    releases = [v for v in versions if v["version_type"] == "release"]
    featured = [v for v in releases if v["featured"]]
    if featured:
        return featured[0]
    if releases:
        return releases[0]
    return versions[0]


def primary_file(version: dict) -> dict:
    return next((f for f in version["files"] if f["primary"]), version["files"][0])


def sri_from_modrinth(f: dict) -> str:
    """Convert Modrinth's hex SHA-512 into an SRI string usable by pkgs.fetchurl."""
    sha512_bytes = bytes.fromhex(f["hashes"]["sha512"])
    return "sha512-" + base64.b64encode(sha512_bytes).decode()


def resolve(
    ids_or_slugs: list[str | dict],
    game_versions: list[str],
    loader: str,
    resolved: dict[str, dict] | None = None,
) -> dict[str, dict]:
    """Resolve mod identifiers to slug -> {url} dicts, including transitive required deps."""
    if resolved is None:
        resolved = {}

    seen_pids: set[str] = set()
    queue = list(ids_or_slugs)
    while queue:
        ref = queue.pop(0)
        if isinstance(ref, dict):
            slug = ref["slug"]
            if "hash" not in ref:
                raise ValueError(
                    f"explicit artifact '{slug}' in manifest is missing 'hash'. "
                    f"Compute one with: nix store prefetch-file --json --hash-type sha256 '{ref['url']}' | jq -r .hash"
                )
            resolved[slug] = {
                "url": ref["url"],
                "hash": ref["hash"],
            }
            print(f"  {slug}: explicit artifact", file=sys.stderr)
            continue

        proj = get_project(ref)
        pid = proj["id"]
        if pid in seen_pids:
            continue
        seen_pids.add(pid)

        versions = get_versions(pid, game_versions, loader)
        version = pick_version(versions)
        if version is None:
            print(f"  SKIP {proj['slug']}: no compatible version", file=sys.stderr)
            continue

        f = primary_file(version)
        resolved[proj["slug"]] = {
            "url": f["url"],
            "hash": sri_from_modrinth(f),
        }
        print(f"  {proj['slug']}: {version['name']}", file=sys.stderr)

        for dep in version.get("dependencies", []):
            dep_id = dep.get("project_id")
            if dep.get("dependency_type") == "required" and dep_id and dep_id not in seen_pids:
                queue.append(dep_id)

    return resolved


def generate(manifest_path: Path, output_dir: Path, only_version: str | None):
    manifest = json.loads(manifest_path.read_text())
    loader = manifest["loader"]

    common_cfg = manifest.get("common", {})
    common_slugs = common_cfg.get("mods", [])
    common_game_versions = common_cfg.get("game_versions", [])

    common_slug_set: set[str] = set()
    if common_slugs:
        print("common:", file=sys.stderr)
        common_resolved = resolve(common_slugs, common_game_versions, loader)

        # Evict mods whose picked version doesn't span ALL common game versions.
        # These get resolved per-version instead.
        evicted = []
        for slug in list(common_resolved.keys()):
            proj = get_project(slug)
            versions = get_versions(proj["id"], common_game_versions, loader)
            version = pick_version(versions)
            if version is None:
                evicted.append(slug)
                continue
            supported = set(version["game_versions"])
            missing = [gv for gv in common_game_versions if gv not in supported]
            if missing:
                print(f"  evict {slug}: does not cover {missing}", file=sys.stderr)
                evicted.append(slug)

        for slug in evicted:
            del common_resolved[slug]

        common_slug_set = set(common_resolved.keys())
        write_json(output_dir / "common.json", common_resolved)

    for game_version, slugs in manifest.get("versions", {}).items():
        if only_version and game_version != only_version:
            continue
        print(f"{game_version}:", file=sys.stderr)
        resolved = resolve(slugs, [game_version], loader)
        for slug in common_slug_set:
            resolved.pop(slug, None)
        write_json(output_dir / f"{game_version}.json", resolved)


def write_json(path: Path, catalog: dict[str, dict]):
    """Write {slug: {url, hash}} sorted by slug for deterministic diffs."""
    sorted_catalog = dict(sorted(catalog.items()))
    path.write_text(json.dumps(sorted_catalog, indent=2, sort_keys=True) + "\n")
    print(f"  wrote {path} ({len(catalog)} mods)", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description="Generate Modrinth mod JSON for Nix")
    parser.add_argument("--manifest", type=Path, help="Path to manifest.json")
    parser.add_argument("--output-dir", type=Path, help="Output directory for JSON files")
    parser.add_argument("--version", dest="only_version", help="Only regenerate this game version")
    args = parser.parse_args()

    if args.manifest:
        manifest_path = args.manifest
    else:
        manifest_path = Path(__file__).resolve().parent.parent / "images/games/minecraft/mods/manifest.json"

    output_dir = args.output_dir or manifest_path.parent
    generate(manifest_path, output_dir, args.only_version)


if __name__ == "__main__":
    main()
