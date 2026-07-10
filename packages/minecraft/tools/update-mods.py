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

from pydantic import BaseModel, TypeAdapter

API = "https://api.modrinth.com/v2"
HEADERS = {"User-Agent": "indexable-inc/index update-mods (github.com/indexable-inc/index)"}
SEARCH_PAGE_SIZE = 100

JsonObject = dict[str, Any]

# ---------------------------------------------------------------------------
# Pydantic models for Modrinth API responses.
# Only the fields the code actually reads are declared; extras are ignored.
# ---------------------------------------------------------------------------


class _Hashes(BaseModel):
    sha512: str


class _VersionFile(BaseModel):
    url: str
    filename: str | None = None
    hashes: _Hashes
    size: int | None = None
    primary: bool = False


class _Dependency(BaseModel):
    project_id: str | None = None
    dependency_type: str | None = None


class _Version(BaseModel):
    id: str | None = None
    project_id: str | None = None
    version_number: str | None = None
    name: str | None = None
    version_type: str | None = None
    game_versions: list[str] = []
    loaders: list[str] = []
    date_published: str | None = None
    downloads: int | None = None
    featured: bool = False
    files: list[_VersionFile] = []
    dependencies: list[_Dependency] = []


class _Project(BaseModel):
    id: str | None = None
    project_id: str | None = None
    slug: str
    project_type: str | None = None
    title: str | None = None
    description: str | None = None
    icon_url: str | None = None
    color: int | None = None
    categories: list[str] = []
    additional_categories: list[str] = []
    client_side: str | None = None
    server_side: str | None = None
    downloads: int | None = None
    followers: int | None = None
    issues_url: str | None = None
    source_url: str | None = None
    wiki_url: str | None = None
    discord_url: str | None = None
    donation_urls: list[Any] = []
    license: Any = None
    game_versions: list[str] = []
    loaders: list[str] = []
    published: str | None = None
    date_created: str | None = None
    updated: str | None = None
    date_modified: str | None = None
    gallery: list[JsonObject] = []


class _SearchPage(BaseModel):
    hits: list[JsonObject] = []
    total_hits: int = 0


_VersionList = TypeAdapter(list[_Version])
_ProjectList = TypeAdapter(list[_Project])

_project_cache: dict[str, _Project] = {}
_version_cache: dict[tuple[str, tuple[str, ...], tuple[str, ...]], list[_Version]] = {}


def api_get(path: str, params: JsonObject | None = None) -> object:
    """Fetch and JSON-decode a Modrinth endpoint.

    Returns the raw decoded JSON as `object`; callers pass it to
    `model_validate` / `validate_python` at the boundary.
    """
    url = f"{API}{path}"
    if params:
        url += "?" + urllib.parse.urlencode(params)
    req = urllib.request.Request(url, headers=HEADERS)  # noqa: S310 -- URL is always https:// (API constant + Modrinth artifact URLs)

    for attempt in range(3):
        try:
            with urllib.request.urlopen(req) as resp:  # noqa: S310 -- same: HTTPS-only Modrinth API
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


def get_project(id_or_slug: str) -> _Project:
    if id_or_slug not in _project_cache:
        project = _Project.model_validate(api_get(f"/project/{id_or_slug}"))
        cache_project(project)
    return _project_cache[id_or_slug]


def cache_project(project: _Project) -> None:
    project_id = project.id or project.project_id
    _project_cache[project.slug] = project
    if project_id:
        _project_cache[project_id] = project


def get_projects(ids_or_slugs: list[str]) -> list[_Project]:
    missing = [ref for ref in dict.fromkeys(ids_or_slugs) if ref not in _project_cache]
    for offset in range(0, len(missing), SEARCH_PAGE_SIZE):
        chunk = missing[offset : offset + SEARCH_PAGE_SIZE]
        if not chunk:
            continue
        projects = _ProjectList.validate_python(api_get("/projects", {"ids": json.dumps(chunk)}))
        for project in projects:
            cache_project(project)

    return [get_project(ref) for ref in ids_or_slugs if ref in _project_cache]


def get_versions(project_id: str, game_versions: list[str], loaders: list[str]) -> list[_Version]:
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
        _version_cache[key] = _VersionList.validate_python(
            api_get(f"/project/{project_id}/version", params)
        )
    return _version_cache[key]


def pick_version(versions: list[_Version]) -> _Version | None:
    if not versions:
        return None
    releases = [version for version in versions if version.version_type == "release"]
    featured = [version for version in releases if version.featured]
    if featured:
        return featured[0]
    if releases:
        return releases[0]
    return versions[0]


def primary_file(version: _Version) -> _VersionFile:
    files = version.files
    return next((file for file in files if file.primary), files[0])


def sri_from_modrinth(file: _VersionFile) -> str:
    """Convert Modrinth's hex SHA-512 into an SRI string usable by pkgs.fetchurl."""
    sha512_bytes = bytes.fromhex(file.hashes.sha512)
    return "sha512-" + base64.b64encode(sha512_bytes).decode()


def sri_from_url(url: str) -> str:
    """Download a hand-picked artifact and return a SHA-256 SRI string."""
    req = urllib.request.Request(url, headers=HEADERS)  # noqa: S310 -- explicit-artifact URLs come from the manifest, expected to be https://
    sha256 = hashlib.sha256()
    with urllib.request.urlopen(req) as resp:  # noqa: S310 -- same: manifest-supplied HTTPS URL
        while chunk := resp.read(1024 * 1024):
            sha256.update(chunk)
    return "sha256-" + base64.b64encode(sha256.digest()).decode()


def artifact_lock(file: _VersionFile, extra: JsonObject | None = None) -> JsonObject:
    value: JsonObject = {
        "url": file.url,
        "hash": sri_from_modrinth(file),
    }
    if extra:
        value.update(extra)
    return value


def catalog_extra(ref: JsonObject) -> JsonObject:
    return {key: ref[key] for key in ["pluginName"] if key in ref}


def summarize_file(file: _VersionFile) -> JsonObject:
    return {
        "filename": file.filename,
        "url": file.url,
        "hashes": {"sha512": file.hashes.sha512},
        "size": file.size,
        "primary": file.primary,
    }


def summarize_version(version: _Version, file: _VersionFile) -> JsonObject:
    return compact({
        "id": version.id,
        "project_id": version.project_id,
        "version_number": version.version_number,
        "name": version.name,
        "version_type": version.version_type,
        "game_versions": version.game_versions,
        "loaders": version.loaders,
        "date_published": version.date_published,
        "downloads": version.downloads,
        "file": summarize_file(file),
        "dependencies": [
            {"project_id": d.project_id, "dependency_type": d.dependency_type}
            for d in version.dependencies
        ],
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


def summarize_project(project: _Project) -> JsonObject:
    project_type = project.project_type
    slug = project.slug
    return compact({
        "source": "modrinth",
        "project_id": project.id or project.project_id,
        "slug": slug,
        "project_type": project_type,
        "page_url": f"https://modrinth.com/{project_type}/{slug}" if project_type else None,
        "title": project.title,
        "description": project.description,
        "icon_url": project.icon_url,
        "color": project.color,
        "categories": project.categories,
        "additional_categories": project.additional_categories,
        "client_side": project.client_side,
        "server_side": project.server_side,
        "downloads": project.downloads,
        "followers": project.followers,
        "issues_url": project.issues_url,
        "source_url": project.source_url,
        "wiki_url": project.wiki_url,
        "discord_url": project.discord_url,
        "donation_urls": project.donation_urls,
        "license": project.license,
        "game_versions": project.game_versions,
        "loaders": project.loaders,
        "date_created": project.published or project.date_created,
        "date_modified": project.updated or project.date_modified,
        "gallery": summarize_gallery(project.gallery),
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
    project: _Project,
    selection_key: str,
    version: _Version,
    file: _VersionFile,
) -> None:
    slug = project.slug
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
        pid = project.id or project.project_id or ""
        if pid in seen_pids:
            continue
        seen_pids.add(pid)

        versions = get_versions(pid, game_versions, loaders)
        version = pick_version(versions)
        if version is None:
            print(f"  SKIP {project.slug}: no compatible version", file=sys.stderr)
            continue

        file = primary_file(version)
        resolved[project.slug] = artifact_lock(file, extra)
        remember_selected_version(projects, project, selection_key, version, file)
        print(f"  {project.slug}: {version.name}", file=sys.stderr)

        for dep in version.dependencies:
            dep_id = dep.project_id
            if dep.dependency_type == "required" and dep_id and dep_id not in seen_pids:
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

        page = _SearchPage.model_validate(api_get("/search", params))
        hits = page.hits
        total_hits = page.total_hits
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
        slug = project.slug
        projects.setdefault(slug, summarize_project(project))

        if game_versions and loaders:
            pid = project.id or project.project_id or ""
            versions = get_versions(pid, game_versions, loaders)
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
    *,
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
            pid = project.id or project.project_id or ""
            versions = get_versions(pid, common_game_versions, [loader])
            version = pick_version(versions)
            if version is None:
                evicted.append(slug)
                continue
            supported = set(version.game_versions)
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
    cwd_manifest = Path.cwd() / "packages/minecraft/catalogs/mods/manifest.json"
    if cwd_manifest.exists():
        return cwd_manifest

    return Path(__file__).resolve().parent.parent / "packages/minecraft/catalogs/mods/manifest.json"


def main() -> None:
    parser = argparse.ArgumentParser(description="Generate Minecraft mod catalogs and metadata")
    parser.add_argument("--manifest", type=Path, help="Path to manifest.json")
    parser.add_argument("--output-dir", type=Path, help="Output directory for JSON files")
    parser.add_argument("--version", dest="only_version", help="Only regenerate this game version")
    parser.add_argument("--skip-searches", action="store_true", help="Skip broad Modrinth search indexes")
    args = parser.parse_args()

    manifest_path = args.manifest or default_manifest_path()

    output_dir = args.output_dir or manifest_path.parent
    generate(manifest_path, output_dir, args.only_version, skip_searches=args.skip_searches)


if __name__ == "__main__":
    main()
