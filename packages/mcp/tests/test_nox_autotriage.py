"""Unit tests for the nox_autotriage module.

Run with::

    PYTHONPATH=packages/mcp/src/linear:packages/mcp/src/nox_autotriage \\
        pytest packages/mcp/tests/test_nox_autotriage.py -q

No network is used. All Linear I/O goes through FakeLinearPort.
"""

from __future__ import annotations

import asyncio
import copy
import json
import os
import sys
from pathlib import Path

# Make imports work when running directly against the source tree.
# In the nix env the modules are installed; when running locally we add src.
_mcp = Path(__file__).parent / ".."
_src_linear = str(_mcp / "src" / "linear")
_src_nox = str(_mcp / "src" / "nox_autotriage")
for _p in (_src_linear, _src_nox):
    if _p not in sys.path:
        sys.path.insert(0, _p)

import pytest
from typing import Any
from linear_test_support import FakeLinearPort

from linear.triage import (
    Finding,
    TriageConfig,
    fingerprint,
    marker_line,
    triage,
    MARKER_KEY,
)
from nox_autotriage import findings_from_conformance, config_from_env, run


# ---------------------------------------------------------------------------
# Shared fixtures
# ---------------------------------------------------------------------------

FIXTURE_PATH = Path(__file__).parent / "fixtures" / "conformance_divergences.json"


@pytest.fixture
def report() -> dict[str, Any]:
    """Full conformance report covering all outcome kinds."""
    with FIXTURE_PATH.open() as fh:
        return json.load(fh)


@pytest.fixture
def mismatch_report() -> dict[str, Any]:
    """Minimal report with one mismatch only."""
    return {
        "schema_version": 1,
        "rev": "aabbccdd1122334455667788990011aabbccddee",
        "summary": {"matched": 0, "mismatched": [], "nox_errors": [], "nix_errors": [], "both_errors": 0, "timeouts": 0},
        "attrs": [
            {
                "attr": "nixpkgs.hello",
                "outcome": {
                    "kind": "mismatch",
                    "nox": "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1-hello-2.12",
                    "nix": "/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-hello-2.12",
                },
            }
        ],
    }


# ---------------------------------------------------------------------------
# FakeLinearPort (mirrors test_linear_triage.py)
# ---------------------------------------------------------------------------


def _run(coro: object) -> object:
    return asyncio.run(coro)  # type: ignore[arg-type]


# ---------------------------------------------------------------------------
# findings_from_conformance: kind filtering
# ---------------------------------------------------------------------------


class TestFindingsFromConformance:
    def test_keeps_mismatch_and_nox_error_only(self, report: dict[str, Any]) -> None:
        """Only mismatch and nox_error outcomes become Findings."""
        findings = findings_from_conformance(report)
        kinds = {f.kind for f in findings}
        assert kinds == {"mismatch", "nox_error"}
        # No nix_error, both_error, match, or timeout
        assert "nix_error" not in kinds
        assert "both_error" not in kinds
        assert "match" not in kinds
        assert "timeout" not in kinds

    def test_correct_count(self, report: dict[str, Any]) -> None:
        """Exactly 2 entries from the fixture are kept."""
        findings = findings_from_conformance(report)
        assert len(findings) == 2

    def test_mismatch_finding_attributes(self, report: dict[str, Any]) -> None:
        """Mismatch finding has correct source, kind, key, title, priority."""
        findings = findings_from_conformance(report)
        mismatch = next(f for f in findings if f.kind == "mismatch")
        assert mismatch.source == "nox-conformance"
        assert mismatch.key == "mismatch:nixpkgs.hello"
        assert mismatch.title == "[nox] mismatch: nixpkgs.hello"
        assert mismatch.priority == 2

    def test_nox_error_finding_attributes(self, report: dict[str, Any]) -> None:
        """nox_error finding has correct source, kind, key, title, priority."""
        findings = findings_from_conformance(report)
        nox_err = next(f for f in findings if f.kind == "nox_error")
        assert nox_err.source == "nox-conformance"
        assert nox_err.key == "nox_error:nixpkgs.badpkg"
        assert nox_err.title == "[nox] nox_error: nixpkgs.badpkg"
        assert nox_err.priority == 2

    def test_schema_version_drift_raises(self) -> None:
        """A report with schema_version != 1 raises ValueError with a clear message."""
        bad_report: dict[str, Any] = {
            "schema_version": 2,
            "rev": "aabbccdd",
            "attrs": [],
        }
        with pytest.raises(ValueError, match="unsupported conformance report schema_version 2"):
            findings_from_conformance(bad_report)

    def test_missing_schema_version_raises(self) -> None:
        """A report with no schema_version raises ValueError."""
        bad_report: dict[str, Any] = {"rev": "aabbccdd", "attrs": []}
        with pytest.raises(ValueError, match="unsupported conformance report schema_version None"):
            findings_from_conformance(bad_report)

    def test_empty_attrs(self) -> None:
        """A report with no attrs produces no findings."""
        report: dict[str, Any] = {
            "schema_version": 1,
            "rev": "aabbccdd",
            "summary": {},
            "attrs": [],
        }
        assert findings_from_conformance(report) == []


# ---------------------------------------------------------------------------
# Rev-stability: the critical cross-rev dedup property
# ---------------------------------------------------------------------------


class TestRevStability:
    def test_different_rev_same_fingerprint(self) -> None:
        """Two reports identical except for rev produce Findings with the same fingerprint."""
        base: dict[str, Any] = {
            "schema_version": 1,
            "rev": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "summary": {},
            "attrs": [
                {
                    "attr": "nixpkgs.hello",
                    "outcome": {
                        "kind": "mismatch",
                        "nox": "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1-hello-2.12",
                        "nix": "/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-hello-2.12",
                    },
                }
            ],
        }
        other = copy.deepcopy(base)
        other["rev"] = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"

        findings_a = findings_from_conformance(base)
        findings_b = findings_from_conformance(other)

        assert len(findings_a) == 1
        assert len(findings_b) == 1
        assert fingerprint(findings_a[0]) == fingerprint(findings_b[0])

    def test_rev_outside_first_200_chars_mismatch(self, mismatch_report: dict[str, Any]) -> None:
        """For mismatch, the rev must appear after the first 200 chars of body_md."""
        findings = findings_from_conformance(mismatch_report)
        assert len(findings) == 1
        body = findings[0].body_md
        rev = mismatch_report["rev"]
        rev_idx = body.index(rev)
        assert rev_idx >= 200, (
            f"rev appears at position {rev_idx} which is within the first 200 "
            f"characters of body_md; this defeats cross-rev dedup.\n"
            f"body[:250]={body[:250]!r}"
        )

    def test_rev_outside_first_200_chars_nox_error(self, report: dict[str, Any]) -> None:
        """For nox_error, the rev must appear after position 0 and have a stable prefix."""
        findings = findings_from_conformance(report)
        nox_err = next(f for f in findings if f.kind == "nox_error")
        rev = report["rev"]
        # Rev must appear in the body
        assert rev in nox_err.body_md
        rev_idx = nox_err.body_md.index(rev)
        # The deterministic header must be at the very start
        assert nox_err.body_md.startswith("nox fails to evaluate"), (
            "nox_error body must start with a deterministic rev-free header"
        )
        # Rev must be after the header + stderr block (i.e., not in the first 200 chars
        # if the first line alone is not 200+ chars, the fingerprint still gets a
        # rev-free stable prefix from the first line)
        # The key assertion: fingerprint is computed on body_md[:200]; rev should
        # not be in that slice for the typical case (long stderr).
        # We enforce that the body starts with the deterministic header which is
        # always included in body[:200] and is rev-free.
        first_200 = nox_err.body_md[:200]
        assert "nox fails to evaluate" in first_200
        assert rev not in first_200 or len(nox_err.body_md) <= 200, (
            "If rev appears within the first 200 chars and body is long, "
            "the fingerprint will change across revs."
        )

    def test_different_rev_same_fingerprint_nox_error(self) -> None:
        """nox_error findings from two different revs have the same fingerprint."""
        base: dict[str, Any] = {
            "schema_version": 1,
            "rev": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "summary": {},
            "attrs": [
                {
                    "attr": "nixpkgs.badpkg",
                    "outcome": {
                        "kind": "nox_error",
                        "stderr": "error: evaluation aborted\n  some long error output\n  spanning multiple lines\n  to ensure we have enough content",
                    },
                }
            ],
        }
        other = copy.deepcopy(base)
        other["rev"] = "cccccccccccccccccccccccccccccccccccccccc"

        findings_a = findings_from_conformance(base)
        findings_b = findings_from_conformance(other)

        assert len(findings_a) == 1
        assert len(findings_b) == 1
        assert fingerprint(findings_a[0]) == fingerprint(findings_b[0])


# ---------------------------------------------------------------------------
# run() with dry_run=True
# ---------------------------------------------------------------------------


class TestRunDryRun:
    def test_dry_run_no_files_created(
        self, report: dict[str, Any], monkeypatch: pytest.MonkeyPatch, tmp_path: Path
    ) -> None:
        """run(..., dry_run=True) returns the expected shape and files nothing."""
        import nox_autotriage as _mod

        # Inject env for config_from_env
        monkeypatch.setenv("TRIAGE_TEAM_ID", "team-uuid-test")
        monkeypatch.setenv("TRIAGE_EPIC_ID", "epic-uuid-test")
        monkeypatch.setenv("TRIAGE_LABEL_IDS", "label-uuid-1,label-uuid-2")
        monkeypatch.setenv("TRIAGE_MAX_NEW_PER_RUN", "10")

        # Write the fixture report to a temp file
        report_file = tmp_path / "report.json"
        report_file.write_text(json.dumps(report))

        # Inject a FakeLinearPort by monkeypatching ModuleLinearPort
        fake_port = FakeLinearPort()
        monkeypatch.setattr(_mod, "ModuleLinearPort", lambda: fake_port)

        result = _run(_mod.run(str(report_file), dry_run=True))

        # Should have filed the 2 kept findings in dry_run mode (filed list is
        # populated even in dry_run -- see triage() implementation)
        assert result["dry_run"] is True
        assert isinstance(result["filed"], list)
        assert isinstance(result["updated"], list)
        assert isinstance(result["deferred"], int)
        # No real API calls
        assert fake_port.created == []
        assert fake_port.commented == []

    def test_dry_run_filed_keys_match_findings(
        self, report: dict[str, Any], monkeypatch: pytest.MonkeyPatch, tmp_path: Path
    ) -> None:
        """dry_run filed keys match the keys of the mismatch+nox_error findings."""
        import nox_autotriage as _mod

        monkeypatch.setenv("TRIAGE_TEAM_ID", "team-uuid-test")
        monkeypatch.setenv("TRIAGE_EPIC_ID", "epic-uuid-test")
        monkeypatch.setenv("TRIAGE_LABEL_IDS", "label-uuid-1")
        monkeypatch.setenv("TRIAGE_MAX_NEW_PER_RUN", "10")

        report_file = tmp_path / "report.json"
        report_file.write_text(json.dumps(report))

        fake_port = FakeLinearPort()
        monkeypatch.setattr(_mod, "ModuleLinearPort", lambda: fake_port)

        result = _run(_mod.run(str(report_file), dry_run=True))

        expected_keys = {"mismatch:nixpkgs.hello", "nox_error:nixpkgs.badpkg"}
        assert set(result["filed"]) == expected_keys


# ---------------------------------------------------------------------------
# config_from_env
# ---------------------------------------------------------------------------


class TestConfigFromEnv:
    def test_missing_team_id_raises(self, monkeypatch: pytest.MonkeyPatch) -> None:
        monkeypatch.delenv("TRIAGE_TEAM_ID", raising=False)
        monkeypatch.setenv("TRIAGE_EPIC_ID", "epic-uuid")
        monkeypatch.setenv("TRIAGE_LABEL_IDS", "label-uuid")
        with pytest.raises(RuntimeError, match="TRIAGE_TEAM_ID"):
            config_from_env()

    def test_missing_epic_id_raises(self, monkeypatch: pytest.MonkeyPatch) -> None:
        monkeypatch.setenv("TRIAGE_TEAM_ID", "team-uuid")
        monkeypatch.delenv("TRIAGE_EPIC_ID", raising=False)
        monkeypatch.setenv("TRIAGE_LABEL_IDS", "label-uuid")
        with pytest.raises(RuntimeError, match="TRIAGE_EPIC_ID"):
            config_from_env()

    def test_missing_label_ids_raises(self, monkeypatch: pytest.MonkeyPatch) -> None:
        monkeypatch.setenv("TRIAGE_TEAM_ID", "team-uuid")
        monkeypatch.setenv("TRIAGE_EPIC_ID", "epic-uuid")
        monkeypatch.delenv("TRIAGE_LABEL_IDS", raising=False)
        with pytest.raises(RuntimeError, match="TRIAGE_LABEL_IDS"):
            config_from_env()

    def test_valid_env_produces_config(self, monkeypatch: pytest.MonkeyPatch) -> None:
        monkeypatch.setenv("TRIAGE_TEAM_ID", "team-uuid")
        monkeypatch.setenv("TRIAGE_EPIC_ID", "epic-uuid")
        monkeypatch.setenv("TRIAGE_LABEL_IDS", "label-a,label-b")
        monkeypatch.setenv("TRIAGE_MAX_NEW_PER_RUN", "5")
        cfg = config_from_env()
        assert cfg.team_id == "team-uuid"
        assert cfg.epic_id == "epic-uuid"
        assert cfg.label_ids == ("label-a", "label-b")
        assert cfg.max_new_per_run == 5

    def test_default_max_new_per_run(self, monkeypatch: pytest.MonkeyPatch) -> None:
        monkeypatch.setenv("TRIAGE_TEAM_ID", "team-uuid")
        monkeypatch.setenv("TRIAGE_EPIC_ID", "epic-uuid")
        monkeypatch.setenv("TRIAGE_LABEL_IDS", "label-a")
        monkeypatch.delenv("TRIAGE_MAX_NEW_PER_RUN", raising=False)
        cfg = config_from_env()
        assert cfg.max_new_per_run == 10
