"""nox-aware adapter: converts a nox conformance report into linear.triage Findings.

Typical usage::

    import asyncio
    from nox_autotriage import findings_from_conformance, run

    # Direct use
    with open("conformance.json") as f:
        report = json.load(f)
    findings = findings_from_conformance(report)

    # Or via the async entry point
    result = asyncio.run(run("conformance.json", dry_run=True))
"""

from __future__ import annotations

import asyncio
import json
import os
import pathlib
from typing import Any

from linear.triage import Finding, TriageConfig, ModuleLinearPort, triage

__all__ = [
    "SCHEMA_VERSION",
    "config_from_env",
    "findings_from_conformance",
    "run",
]

SCHEMA_VERSION = 1


def findings_from_conformance(report: dict[str, Any]) -> list[Finding]:
    """Convert a nox conformance JSON report into a list of triage Findings.

    Only ``mismatch`` and ``nox_error`` outcomes are converted to Findings.
    All other outcome kinds (match, nix_error, both_error, timeout) are
    silently dropped -- they do not represent actionable nox regressions.

    The ``rev`` field is placed past the first 200 characters of ``body_md``
    so that cross-rev deduplication works: the ``fingerprint`` function hashes
    only the first 200 chars, and the leading content uses store paths (which
    normalise away their hashes) or deterministic per-attr headers, keeping
    the fingerprint rev-stable across nixpkgs bumps.

    Raises ValueError if the report schema_version is not the expected value.
    """
    if report.get("schema_version") != SCHEMA_VERSION:
        raise ValueError(
            f"unsupported conformance report schema_version "
            f"{report.get('schema_version')!r}, expected {SCHEMA_VERSION}"
        )

    rev: str = report["rev"]
    findings: list[Finding] = []

    for entry in report.get("attrs", []):
        attr: str = entry["attr"]
        outcome: dict[str, Any] = entry["outcome"]
        kind: str = outcome["kind"]

        if kind == "mismatch":
            nox_path: str = outcome["nox"]
            nix_path: str = outcome["nix"]
            # Body: stable divergence detail first (store paths normalise so
            # they are rev-stable), rev placed after 200+ chars.
            # Two store paths comfortably exceed 200 chars, so `rev` is
            # always outside the fingerprinted prefix.
            body_md = (
                f"nox and nix disagree on the output path for `{attr}`.\n\n"
                f"nox: `{nox_path}`\n"
                f"nix: `{nix_path}`\n\n"
                f"Pinned nixpkgs rev: {rev}\n\n"
                f"Root-cause: `nox-conformance --explain {attr}`"
            )
            findings.append(
                Finding(
                    source="nox-conformance",
                    kind=kind,
                    key=f"{kind}:{attr}",
                    title=f"[nox] {kind}: {attr}",
                    body_md=body_md,
                    priority=2,
                )
            )

        elif kind == "nox_error":
            stderr: str = outcome.get("stderr", "")
            # Body: deterministic rev-free header on the first line so the
            # fingerprint has a stable anchor regardless of stderr length,
            # then the stderr block, then rev.
            body_md = (
                f"nox fails to evaluate `{attr}` (nox_error).\n\n"
                f"stderr:\n```\n{stderr}\n```\n\n"
                f"Pinned nixpkgs rev: {rev}\n\n"
                f"Root-cause: `nox-conformance --explain {attr}`"
            )
            findings.append(
                Finding(
                    source="nox-conformance",
                    kind=kind,
                    key=f"{kind}:{attr}",
                    title=f"[nox] {kind}: {attr}",
                    body_md=body_md,
                    priority=2,
                )
            )

    return findings


def config_from_env() -> TriageConfig:
    """Build a TriageConfig from environment variables.

    Required variables:
        TRIAGE_TEAM_ID     Linear team UUID.
        TRIAGE_EPIC_ID     Linear parent issue (epic) UUID.
        TRIAGE_LABEL_IDS   Comma-separated label UUIDs.

    Optional variables:
        TRIAGE_MAX_NEW_PER_RUN   Maximum new issues per run (default 10).

    Raises RuntimeError if a required variable is missing or empty.
    """
    def _require(name: str) -> str:
        val = os.environ.get(name, "").strip()
        if not val:
            raise RuntimeError(
                f"Required environment variable {name!r} is not set. "
                "Inject it via the workflow env block."
            )
        return val

    team_id = _require("TRIAGE_TEAM_ID")
    epic_id = _require("TRIAGE_EPIC_ID")
    label_ids_raw = _require("TRIAGE_LABEL_IDS")
    label_ids = tuple(lbl.strip() for lbl in label_ids_raw.split(",") if lbl.strip())

    max_new_raw = os.environ.get("TRIAGE_MAX_NEW_PER_RUN", "10").strip()
    try:
        max_new = int(max_new_raw)
    except ValueError as exc:
        raise RuntimeError(
            f"TRIAGE_MAX_NEW_PER_RUN must be an integer, got {max_new_raw!r}"
        ) from exc

    return TriageConfig(
        team_id=team_id,
        epic_id=epic_id,
        label_ids=label_ids,
        max_new_per_run=max_new,
    )


async def run(report_path: str, *, dry_run: bool) -> dict[str, Any]:
    """Load a conformance report, derive findings, and triage them to Linear.

    Args:
        report_path: Filesystem path to the JSON conformance report.
        dry_run:     When True, decisions are computed but no Linear API calls
                     are made (same behaviour as linear.triage.triage dry_run).

    Returns a plain dict with keys: filed, updated, deferred, dry_run.
    """
    report = await asyncio.to_thread(
        lambda: json.loads(pathlib.Path(report_path).read_text())
    )

    findings = findings_from_conformance(report)
    cfg = config_from_env()
    port = ModuleLinearPort()
    result = await triage(findings, cfg, port, dry_run=dry_run)
    return {
        "filed": result.filed,
        "updated": result.updated,
        "deferred": result.deferred,
        "dry_run": result.dry_run,
    }
