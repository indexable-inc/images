"""Source-agnostic dedup and triage core for Linear issue filing.

This module converts a list of :class:`Finding` objects into Linear issues,
with fingerprint-based deduplication to prevent duplicate filings across runs.
It has no knowledge of nox or any specific test framework: findings arrive as
plain dataclass instances and the module delegates all Linear I/O through a
:class:`LinearPort` protocol so tests can inject a fake without network access.

Typical usage::

    from linear.triage import Finding, TriageConfig, ModuleLinearPort, triage

    cfg = TriageConfig(
        team_id="<team-uuid>",
        epic_id="<parent-issue-uuid>",
        label_ids=("label-uuid-1",),
        max_new_per_run=10,
    )
    findings = [
        Finding(
            source="ci",
            kind="lint",
            key="src/foo.rs:unused_import",
            title="Unused import in src/foo.rs",
            body_md="The import on line 4 is unused.",
            priority=3,
        ),
    ]
    result = await triage(findings, cfg, ModuleLinearPort(), dry_run=False)
    print(result.filed, result.updated, result.deferred)
"""

from __future__ import annotations

import hashlib
import re
from dataclasses import dataclass, field
from typing import Any, Mapping, Protocol, runtime_checkable

__all__ = [
    "Finding",
    "TriageConfig",
    "TriageResult",
    "LinearPort",
    "ModuleLinearPort",
    "fingerprint",
    "marker_line",
    "triage",
    "MARKER_KEY",
]

# ---------------------------------------------------------------------------
# Finding
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class Finding:
    """An individual finding to be filed (or deduplicated) in Linear.

    Fields
    ------
    source:
        Logical origin of the finding, e.g. ``"ci"``, ``"antithesis"``.
    kind:
        Category within the source, e.g. ``"lint"``, ``"property-failure"``.
    key:
        Stable identifier for this specific defect.  Two runs that observe
        the same bug should produce the same ``key`` after normalisation.
    title:
        Short human-readable summary, used as the Linear issue title.
    body_md:
        Full description in Markdown, used as the issue body.
    priority:
        Linear priority: 1 = Urgent, 2 = High, 3 = Medium, 4 = Low,
        0 = No priority (treated as lowest urgency when sorting).
    marker_fields:
        Optional extra key-value pairs embedded in the fingerprint.
        Use a tuple of 2-tuples instead of a dict so the dataclass stays
        frozen and hashable, e.g.
        ``marker_fields=(("team", "ENG"), ("run", "nightly"))``.
    """

    source: str
    kind: str
    key: str
    title: str
    body_md: str
    priority: int = 3
    marker_fields: tuple[tuple[str, str], ...] = ()

    @classmethod
    def from_mapping(
        cls,
        source: str,
        kind: str,
        key: str,
        title: str,
        body_md: str,
        priority: int = 3,
        marker_fields: Mapping[str, str] | None = None,
    ) -> "Finding":
        """Convenience constructor that accepts a plain Mapping for marker_fields.

        ``marker_fields`` is converted to a sorted tuple of pairs so the
        resulting :class:`Finding` is deterministic regardless of mapping
        iteration order.
        """
        pairs: tuple[tuple[str, str], ...] = ()
        if marker_fields:
            pairs = tuple(sorted(marker_fields.items()))
        return cls(
            source=source,
            kind=kind,
            key=key,
            title=title,
            body_md=body_md,
            priority=priority,
            marker_fields=pairs,
        )


# ---------------------------------------------------------------------------
# Normalisation and fingerprinting
# ---------------------------------------------------------------------------


def _normalize(s: str) -> str:
    """Collapse run-to-run noise so the same defect yields the same fingerprint.

    Strips or replaces:
    - Nix store hashes (``/nix/store/<32-char-hash>-``).
    - Conformance temp store dir suffixes (``nox-conformance-store-<pid>``).
    - Source locations (``:<line>:<col>``).
    - Other ``/tmp/...`` temp paths.
    - Bare ``pid <number>`` references.

    Remaining whitespace is collapsed so minor formatting differences do not
    affect the digest.
    """
    s = re.sub(r"/nix/store/[a-z0-9]{32}-", "/nix/store/<hash>-", s)
    s = re.sub(r"nox-conformance-store-\d+", "nox-conformance-store-<pid>", s)
    s = re.sub(r":\d+:\d+", ":<lc>", s)
    s = re.sub(r"/tmp/[^\s:'\"]+", "/tmp/<tmp>", s)
    s = re.sub(r"\bpid \d+", "pid <pid>", s)
    # Collapse whitespace (tabs, newlines, multiple spaces).
    s = re.sub(r"\s+", " ", s).strip()
    return s


#: Number of hex characters returned by :func:`fingerprint`.  The full
#: SHA-256 digest is 64 chars; 16 gives 64 bits of collision resistance which
#: is sufficient for the dedup use-case while keeping marker lines short.
_FP_LENGTH = 16

MARKER_KEY = "nox-fingerprint"


def fingerprint(f: Finding) -> str:
    """Return a stable, 16-character hex fingerprint for ``f``.

    The digest covers ``source``, ``kind``, the normalised ``key``, and the
    normalised first 200 characters of ``body_md``.  Run-to-run noise (store
    hashes, temp paths, line numbers, pids) is stripped before hashing so two
    observations of the same defect produce the same fingerprint even if the
    paths or line numbers changed.

    Returns the first :data:`_FP_LENGTH` hex digits of the SHA-256 digest.
    """
    payload = "\n".join(
        [
            f.source,
            f.kind,
            _normalize(f.key),
            _normalize(f.body_md[:200]),
        ]
    )
    return hashlib.sha256(payload.encode()).hexdigest()[:_FP_LENGTH]


def marker_line(fp: str) -> str:
    """Return the canonical marker line for fingerprint ``fp``.

    This line is appended to every created issue's body and is used as the
    authoritative dedup key when searching for existing issues.  Example::

        nox-fingerprint: a1b2c3d4e5f60718
    """
    return f"{MARKER_KEY}: {fp}"


# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class TriageConfig:
    """Configuration for a single :func:`triage` run.

    Fields
    ------
    team_id:
        Linear team UUID under which new issues are created.
    epic_id:
        UUID of the parent issue (epic) that new issues are linked to.
    label_ids:
        Tuple of label UUIDs applied to every new issue.
    max_new_per_run:
        Cap on the number of new issues created per run.  Findings beyond
        this cap are counted in :attr:`TriageResult.deferred` rather than
        silently dropped.
    """

    team_id: str
    epic_id: str
    label_ids: tuple[str, ...]
    max_new_per_run: int = 10


# ---------------------------------------------------------------------------
# LinearPort protocol
# ---------------------------------------------------------------------------


@runtime_checkable
class LinearPort(Protocol):
    """Async interface over Linear used by :func:`triage`.

    Concrete implementations delegate to the real ``linear`` module
    (:class:`ModuleLinearPort`) or to a fake for tests.
    """

    async def search(self, term: str) -> list[dict[str, Any]]:
        """Search issues and return a list of issue dicts.

        Each dict must contain at least ``id``, ``title``, ``description``
        (may be ``None``), and ``state`` with a ``type`` string.
        """
        ...

    async def create(
        self,
        *,
        team_id: str,
        title: str,
        description: str,
        parent_id: str,
        label_ids: list[str],
        priority: int,
    ) -> dict[str, Any]:
        """Create a new issue and return the created issue dict."""
        ...

    async def comment(self, issue_id: str, body: str) -> dict[str, Any]:
        """Add a comment to ``issue_id`` and return the comment dict."""
        ...


class ModuleLinearPort:
    """Concrete :class:`LinearPort` backed by the ``linear`` module functions.

    This is the only class in ``triage`` that imports from ``linear``.  Keep
    it thin: one call per method, no business logic.
    """

    async def search(self, term: str) -> list[dict[str, Any]]:
        """Delegate to :func:`linear.issue_search`."""
        import linear

        return await linear.issue_search(term)

    async def create(
        self,
        *,
        team_id: str,
        title: str,
        description: str,
        parent_id: str,
        label_ids: list[str],
        priority: int,
    ) -> dict[str, Any]:
        """Delegate to :func:`linear.issue_create`."""
        import linear

        return await linear.issue_create(
            team_id,
            title,
            description=description,
            parentId=parent_id,
            labelIds=label_ids,
            priority=priority,
        )

    async def comment(self, issue_id: str, body: str) -> dict[str, Any]:
        """Delegate to :func:`linear.comment_create`."""
        import linear

        return await linear.comment_create(issue_id, body)


# ---------------------------------------------------------------------------
# Result
# ---------------------------------------------------------------------------


@dataclass
class TriageResult:
    """Summary of a :func:`triage` run.

    Fields
    ------
    filed:
        Keys of findings for which new Linear issues were created (or would
        have been created in a dry run).
    updated:
        Keys of findings whose existing Linear issues received a comment.
    deferred:
        Count of new findings not created because ``max_new_per_run`` was
        reached.
    dry_run:
        Whether this was a dry run (no actual API calls made).
    """

    filed: list[str] = field(default_factory=list)
    updated: list[str] = field(default_factory=list)
    deferred: int = 0
    dry_run: bool = False


# ---------------------------------------------------------------------------
# Triage
# ---------------------------------------------------------------------------

_OPEN_TYPES: frozenset[str] = frozenset()
_CLOSED_TYPES: frozenset[str] = frozenset({"completed", "canceled"})


def _is_open(state: dict[str, Any]) -> bool:
    """Return True when the state type indicates the issue is still open."""
    return state.get("type", "").lower() not in _CLOSED_TYPES


async def triage(
    findings: list[Finding],
    cfg: TriageConfig,
    port: LinearPort,
    *,
    dry_run: bool,
) -> TriageResult:
    """Deduplicate and file ``findings`` as Linear issues.

    Decision table per finding:

    1. Compute ``fp = fingerprint(finding)`` and ``marker = marker_line(fp)``.
    2. Search for existing issues containing ``marker`` in their description.
    3. If a marker-match exists:
       - Open state: post a bump comment; add to :attr:`TriageResult.updated`.
       - Closed/cancelled: post a regression comment; add to ``updated``.
    4. If no marker-match: search by exact title.  If an exact-title match
       exists, bump-comment it instead of creating a duplicate.
    5. Remaining new findings are sorted by priority (1 = most urgent, 0 =
       least urgent) and up to ``cfg.max_new_per_run`` are created.  The rest
       increment :attr:`TriageResult.deferred`.

    When ``dry_run`` is ``True`` the same decisions are made but no
    ``create``/``comment`` API calls are issued.
    """
    result = TriageResult(dry_run=dry_run)
    new_findings: list[Finding] = []

    for f in findings:
        fp = fingerprint(f)
        marker = marker_line(fp)

        # --- Step 2: marker-based search ---
        # Search the bare 16-hex fingerprint (a distinctive token) rather than
        # the full marker line. Linear's search is fuzzy and capped at
        # ``first``; the bare fingerprint ranks the marker-bearing issue at the
        # top of the window. The exact-substring check against ``marker`` is
        # preserved so a noise hit can never cause a false dedup.
        hits = await port.search(fp)
        marker_match: dict[str, Any] | None = None
        for hit in hits:
            desc = hit.get("description") or ""
            if marker in desc:
                marker_match = hit
                break

        if marker_match is not None:
            issue_id = marker_match["id"]
            state = marker_match.get("state") or {}
            if _is_open(state):
                note = (
                    f"Seen again: source={f.source}, kind={f.kind}, key={f.key}"
                )
            else:
                note = (
                    f"Regression: issue re-opened. "
                    f"source={f.source}, kind={f.kind}, key={f.key}"
                )
            if not dry_run:
                await port.comment(issue_id, note)
            result.updated.append(f.key)
            continue

        # --- Step 4: exact-title guard ---
        title_hits = await port.search(f.title)
        title_match: dict[str, Any] | None = None
        for hit in title_hits:
            if hit.get("title", "") == f.title:
                title_match = hit
                break

        if title_match is not None:
            issue_id = title_match["id"]
            note = (
                f"Seen again (title match): "
                f"source={f.source}, kind={f.kind}, key={f.key}"
            )
            if not dry_run:
                await port.comment(issue_id, note)
            result.updated.append(f.key)
            continue

        # --- Step 5: new ---
        new_findings.append(f)

    # Sort new findings by priority: 1 most urgent, 0 least urgent.
    def _sort_key(f: Finding) -> int:
        # Treat priority 0 (no priority) as lower urgency than 4 (low).
        return f.priority if f.priority > 0 else 5

    new_findings.sort(key=_sort_key)

    to_create = new_findings[: cfg.max_new_per_run]
    deferred = new_findings[cfg.max_new_per_run :]

    for f in to_create:
        fp = fingerprint(f)
        marker = marker_line(fp)
        body = f.body_md + "\n\n" + marker
        if not dry_run:
            await port.create(
                team_id=cfg.team_id,
                title=f.title,
                description=body,
                parent_id=cfg.epic_id,
                label_ids=list(cfg.label_ids),
                priority=f.priority,
            )
        result.filed.append(f.key)

    if deferred:
        count = len(deferred)
        result.deferred += count
        print(
            f"triage: {count} finding(s) deferred (max_new_per_run="
            f"{cfg.max_new_per_run} reached)."
        )

    return result
