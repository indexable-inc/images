#!/usr/bin/env python3
"""Query Modrinth and generate Minecraft artifact catalogs."""

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

API = "https://api.modrinth.com/v2"
HEADERS = {"User-Agent": "indexable-inc/index update-mods (github.com/indexable-inc/index)"}
SEARCH_PAGE_SIZE = 100

JsonObject = dict[str, Any]

_project_cache: dict[str, JsonObject] = {}
_version_cache: dict[tuple[str, tuple[str, ...], tuple[str, ...]], list[JsonObject]] = {}


def api_get(path: str, params: JsonObject | None = None) -> Any:
    url = f"{API}{path}"
    if params:
        url += "?" + urllib.parse.urlencode(params)
    req = urllib.request.Request(url, headers=HEADERS)

    for attempt in range(3):
        try:
            with urllib.request.urlopen(req) as resp:
                if resp.status == 429:
                    time.sleep(2**attempt)
                    continue
                return json.loads(resp.read())
        except urllib.error.HTTPError as err:
            if err.code == 429 and attempt < 2:
                time.sleep(2**attempt)
                continue
            raise

    raise RuntimeError(f"rate limited after retries: {url}")


def get_project(id_or_slug: str) -> JsonObject:
    if id_or_slug not in _project_cache:
        project = api_get(f"/project/{id_or_slug}")
        cache_project(project)
    return _project_cache[id_or_slug]


def cache_project(project: JsonObject) -> None:
    project_id = project.get("id") or project.get("project_id")
    slug = project["slug"]
    _project_cache[slug] = project
    if project_id:
        _project_cache[project_id] = project


def get_projects(ids_or_slugs: list[str]) -> list[JsonObject]:
    missing = [ref for ref in dict.fromkeys(ids_or_slugs) if ref not in _project_cache]
    for offset in range(0, len(missing), SEARCH_PAGE_SIZE):
        chunk = missing[offset : offset + SEARCH_PAGE_SIZE]
        if not chunk:
            continue
        projects = api_get("/projects", {"ids": json.dumps(chunk)})
        for project in projects:
            cache_project(project)

    return [get_project(ref) for ref in ids_or_slugs if ref in _project_cache]


def get_versions(project_id: str, game_versions: list[str], loaders: list[str]) -> list[JsonObject]:
    key = (project_id, tuple(game_versions), tuple(loaders))
    if key not in _version_cache:
        # Modrinth's /version endpoint treats an empty filter array as "match
        # nothing" rather than "no filter", so empty filters must be omitted
        # from the query string entirely. The velocity manifest's common shape
        # has no per-version pin and so passes game_versions=[].
        params: JsonObject = {}
        if game_versions:
            params["game_versions"] = json.dumps(game_versions)
        if loaders:
            params["loaders"] = json.dumps(loaders)
        _version_cache[key] = api_get(f"/project/{project_id}/version", params)
    return _version_cache[key]


def pick_version(versions: list[JsonObject]) -> JsonObject | None:
    if not versions:
        return None
    releases = [version for version in versions if version["version_type"] == "release"]
    featured = [version for version in releases if version["featured"]]
    if featured:
        return featured[0]
    if releases:
        return releases[0]
    return versions[0]


def primary_file(version: JsonObject) -> JsonObject:
    return next((file for file in version["files"] if file["primary"]), version["files"][0])


def sri_from_modrinth(file: JsonObject) -> str:
    """Convert Modrinth's hex SHA-512 into an SRI string usable by pkgs.fetchurl."""
    sha512_bytes = bytes.fromhex(file["hashes"]["sha512"])
    return "sha512-" + base64.b64encode(sha512_bytes).decode()


def sri_from_url(url: str) -> str:
    """Download a hand-picked artifact and return a SHA-256 SRI string."""
    req = urllib.request.Request(url, headers=HEADERS)
    sha256 = hashlib.sha256()
    with urllib.request.urlopen(req) as resp:
        while chunk := resp.read(1024 * 1024):
            sha256.update(chunk)
    return "sha256-" + base64.b64encode(sha256.digest()).decode()


def artifact_lock(file: JsonObject, extra: JsonObject | None = None) -> JsonObject:
    value = {
        "url": file["url"],
        "hash": sri_from_modrinth(file),
    }
    if extra:
        value.update(extra)
    return value


def catalog_extra(ref: JsonObject) -> JsonObject:
    return {key: ref[key] for key in ["pluginName"] if key in ref}


def summarize_file(file: JsonObject) -> JsonObject:
    return {
        "filename": file.get("filename"),
        "url": file.get("url"),
        "hashes": file.get("hashes", {}),
        "size": file.get("size"),
        "primary": file.get("primary", False),
    }


def summarize_version(version: JsonObject, file: JsonObject) -> JsonObject:
    return compact({
        "id": version.get("id"),
        "project_id": version.get("project_id"),
        "version_number": version.get("version_number"),
        "name": version.get("name"),
        "version_type": version.get("version_type"),
        "game_versions": version.get("game_versions", []),
        "loaders": version.get("loaders", []),
        "date_published": version.get("date_published"),
        "downloads": version.get("downloads"),
        "file": summarize_file(file),
        "dependencies": version.get("dependencies", []),
    })


def summarize_gallery(gallery: list[JsonObject]) -> list[JsonObject]:
    return [
        compact({
            "url": item.get("url"),
            "featured": item.get("featured"),
            "title": item.get("title"),
            "description": item.get("description"),
            "created": item.get("created"),
            "ordering": item.get("ordering"),
        })
        for item in gallery
    ]


def summarize_project(project: JsonObject) -> JsonObject:
    project_type = project.get("project_type")
    slug = project["slug"]
    return compact({
        "source": "modrinth",
        "project_id": project.get("id") or project.get("project_id"),
        "slug": slug,
        "project_type": project_type,
        "page_url": f"https://modrinth.com/{project_type}/{slug}" if project_type else None,
        "title": project.get("title"),
        "description": project.get("description"),
        "icon_url": project.get("icon_url"),
        "color": project.get("color"),
        "categories": project.get("categories", []),
        "additional_categories": project.get("additional_categories", []),
        "client_side": project.get("client_side"),
        "server_side": project.get("server_side"),
        "downloads": project.get("downloads"),
        "followers": project.get("followers"),
        "issues_url": project.get("issues_url"),
        "source_url": project.get("source_url"),
        "wiki_url": project.get("wiki_url"),
        "discord_url": project.get("discord_url"),
        "donation_urls": project.get("donation_urls", []),
        "license": project.get("license"),
        "game_versions": project.get("game_versions", []),
        "loaders": project.get("loaders", []),
        "date_created": project.get("published") or project.get("date_created"),
        "date_modified": project.get("updated") or project.get("date_modified"),
        "gallery": summarize_gallery(project.get("gallery", [])),
        "selected_versions": {},
    })


def summarize_explicit_artifact(ref: JsonObject, artifact_hash: str) -> JsonObject:
    slug = ref["slug"]
    return compact({
        "source": "explicit",
        "slug": slug,
        "title": ref.get("title") or slug,
        "description": ref.get("description"),
        "icon_url": ref.get("icon_url"),
        "page_url": ref.get("page_url") or ref.get("url"),
        "selected_versions": {
            "explicit": compact({
                "name": ref.get("name"),
                "version_number": ref.get("version"),
                "file": {
                    "url": ref["url"],
                    "hashes": {
                        "sha256-sri": artifact_hash,
                    },
                },
            }),
        },
    })


def compact(value: JsonObject) -> JsonObject:
    return {
        key: item
        for key, item in value.items()
        if item is not None and item != [] and item != {}
    }


def remember_selected_version(
    projects: dict[str, JsonObject],
    project: JsonObject,
    selection_key: str,
    version: JsonObject,
    file: JsonObject,
) -> None:
    slug = project["slug"]
    projects.setdefault(slug, summarize_project(project))
    projects[slug].setdefault("selected_versions", {})[selection_key] = summarize_version(version, file)


def resolve(
    ids_or_slugs: list[str | JsonObject],
    game_versions: list[str],
    loaders: list[str],
    projects: dict[str, JsonObject],
) -> dict[str, JsonObject]:
    """Resolve identifiers to slug -> {url, hash}, including transitive required deps."""
    resolved: dict[str, JsonObject] = {}
    seen_pids: set[str] = set()
    queue = list(ids_or_slugs)
    selection_key = "+".join(game_versions + loaders)

    while queue:
        ref = queue.pop(0)
        extra: JsonObject = {}
        if isinstance(ref, dict):
            slug = ref["slug"]
            extra = catalog_extra(ref)
            if "url" in ref:
                artifact_hash = sri_from_url(ref["url"])
                resolved[slug] = {
                    "url": ref["url"],
                    "hash": artifact_hash,
                    **extra,
                }
                projects[slug] = summarize_explicit_artifact(ref, artifact_hash)
                print(f"  {slug}: explicit artifact", file=sys.stderr)
                continue

            ref = slug

        project = get_project(ref)
        pid = project["id"]
        if pid in seen_pids:
            continue
        seen_pids.add(pid)

        versions = get_versions(pid, game_versions, loaders)
        version = pick_version(versions)
        if version is None:
            print(f"  SKIP {project['slug']}: no compatible version", file=sys.stderr)
            continue

        file = primary_file(version)
        resolved[project["slug"]] = artifact_lock(file, extra)
        remember_selected_version(projects, project, selection_key, version, file)
        print(f"  {project['slug']}: {version['name']}", file=sys.stderr)

        for dep in version.get("dependencies", []):
            dep_id = dep.get("project_id")
            if dep.get("dependency_type") == "required" and dep_id and dep_id not in seen_pids:
                queue.append(dep_id)

    return resolved


def discover_projects(
    name: str,
    search_config: JsonObject,
    only_version: str | None,
    projects: dict[str, JsonObject],
) -> JsonObject | None:
    game_versions = list(search_config.get("game_versions", []))
    if only_version and game_versions and only_version not in game_versions:
        return None

    loaders = list(search_config.get("loaders", []))
    limit = int(search_config.get("limit", SEARCH_PAGE_SIZE))
    facets = search_config.get("facets", [])
    slugs: list[str] = []
    total_hits = 0

    while len(slugs) < limit:
        page_limit = min(SEARCH_PAGE_SIZE, limit - len(slugs))
        params: JsonObject = {
            "limit": page_limit,
            "offset": len(slugs),
            "index": search_config.get("index", "downloads"),
        }
        if search_config.get("query"):
            params["query"] = search_config["query"]
        if facets:
            params["facets"] = json.dumps(facets)

        page = api_get("/search", params)
        hits = page.get("hits", [])
        total_hits = page.get("total_hits", total_hits)
        if not hits:
            break

        slugs.extend(hit["slug"] for hit in hits)
        if len(hits) < page_limit:
            break

    if not slugs:
        return compact({
            "config": search_config,
            "total_hits": total_hits,
            "slugs": [],
        })

    print(f"{name}: discovered {len(slugs)} of {total_hits} hits", file=sys.stderr)
    for project in get_projects(slugs):
        slug = project["slug"]
        projects.setdefault(slug, summarize_project(project))

        if game_versions and loaders:
            versions = get_versions(project["id"], game_versions, loaders)
            version = pick_version(versions)
            if version is None:
                continue
            file = primary_file(version)
            selection_key = "+".join(game_versions + loaders)
            remember_selected_version(projects, project, selection_key, version, file)

    return compact({
        "config": search_config,
        "total_hits": total_hits,
        "slugs": slugs,
    })


def generate(
    manifest_path: Path,
    output_dir: Path,
    only_version: str | None,
    skip_searches: bool,
) -> None:
    manifest = json.loads(manifest_path.read_text())
    loader = manifest["loader"]
    projects: dict[str, JsonObject] = {}
    searches: dict[str, JsonObject] = {}

    common_cfg = manifest.get("common", {})
    common_slugs = common_cfg.get("mods", [])
    common_game_versions = common_cfg.get("game_versions", [])

    common_slug_set: set[str] = set()
    if common_slugs:
        print("common:", file=sys.stderr)
        common_resolved = resolve(common_slugs, common_game_versions, [loader], projects)

        # Evict mods whose picked version doesn't span ALL common game versions.
        # These get resolved per-version instead. Explicit-artifact entries
        # (those resolved from a manifest `url` rather than a Modrinth slug)
        # have no Modrinth project to query, so the eviction check is skipped.
        evicted = []
        for slug in list(common_resolved.keys()):
            if projects.get(slug, {}).get("source") == "explicit":
                continue
            project = get_project(slug)
            versions = get_versions(project["id"], common_game_versions, [loader])
            version = pick_version(versions)
            if version is None:
                evicted.append(slug)
                continue
            supported = set(version["game_versions"])
            missing = [game_version for game_version in common_game_versions if game_version not in supported]
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
        resolved = resolve(slugs, [game_version], [loader], projects)
        for slug in common_slug_set:
            resolved.pop(slug, None)
        write_json(output_dir / f"{game_version}.json", resolved)

    if not skip_searches:
        for name, search_config in manifest.get("searches", {}).items():
            result = discover_projects(name, search_config, only_version, projects)
            if result is not None:
                searches[name] = result

    write_metadata(output_dir, searches, projects)


def write_metadata(
    output_dir: Path,
    searches: dict[str, JsonObject],
    projects: dict[str, JsonObject],
) -> None:
    metadata = {
        "schema": 1,
        "searches": dict(sorted(searches.items())),
        "projects": dict(sorted(projects.items())),
    }
    metadata_dir = output_dir / "metadata"
    metadata_dir.mkdir(exist_ok=True)
    write_json(metadata_dir / "catalog.json", metadata)


def write_json(path: Path, value: JsonObject) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    print(f"  wrote {path}", file=sys.stderr)


def default_manifest_path() -> Path:
    cwd_manifest = Path.cwd() / "images/games/minecraft/mods/manifest.json"
    if cwd_manifest.exists():
        return cwd_manifest

    return Path(__file__).resolve().parent.parent / "images/games/minecraft/mods/manifest.json"


def main() -> None:
    parser = argparse.ArgumentParser(description="Generate Minecraft mod catalogs and metadata")
    parser.add_argument("--manifest", type=Path, help="Path to manifest.json")
    parser.add_argument("--output-dir", type=Path, help="Output directory for JSON files")
    parser.add_argument("--version", dest="only_version", help="Only regenerate this game version")
    parser.add_argument("--skip-searches", action="store_true", help="Skip broad Modrinth search indexes")
    args = parser.parse_args()

    if args.manifest:
        manifest_path = args.manifest
    else:
        manifest_path = default_manifest_path()

    output_dir = args.output_dir or manifest_path.parent
    generate(manifest_path, output_dir, args.only_version, args.skip_searches)


if __name__ == "__main__":
    main()
