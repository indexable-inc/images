"""Shared record shapes for the distiller's JSON-backed state.

These ``pydantic`` models describe the structures the distiller owns and
persists: distilled items (``Item``), per-session outcome records
(``SessionRecord``), normalized model verdicts (``Verdict``), and the
per-(user, project) state file (``State``). Fields that accrue across runs
(provenance stamps, optional metadata) carry defaults so partial records still
validate while still naming every field a reader may touch.

On-disk JSON compatibility is load-bearing: the state file persists across runs
and is what makes distillation incremental. ``extra="ignore"`` tolerates keys
from older/newer schemas, defaults tolerate missing keys, and ``Item`` omits
its unset provenance stamps on dump (``exclude_none``; see ``state.save``) so
the serialized shape matches the historical ``total=False`` TypedDict output.

Raw model output -- the ``operations`` and ``session_outcomes`` lists returned
by ``claude -p`` -- is intentionally left as ``dict[str, object]`` (see
``distill.py``): it is untrusted external JSON, validated key-by-key at the
merge boundary rather than trusted to match a fixed shape.
"""

from __future__ import annotations

from pydantic import BaseModel, ConfigDict

# A prior run's state file is external (possibly hand-edited or from an older
# schema); ignore unknown keys rather than erroring, matching the old loader.
_TOLERANT = ConfigDict(extra="ignore")


class Item(BaseModel):
    """One distilled ReasoningBank-style lesson, persisted across runs."""

    model_config = _TOLERANT

    id: str
    title: str
    body: str
    # Defaults match the old total=False TypedDict loader's .get() fallbacks in
    # corpus.py ("mixed" / "shared") so legacy state files that omit these
    # fields still validate rather than raising ValidationError.
    outcome: str = "mixed"  # success | failure | mixed
    scope: str = "shared"  # user | shared
    sessions: list[str] = []
    first_seen: float = 0.0
    last_updated: float = 0.0
    # Provenance stamps derived from session metadata; absent (None, omitted on
    # dump) until an item has at least one timestamped evidence session.
    evidence_from: float | None = None
    evidence_to: float | None = None


class SessionRecord(BaseModel):
    """One judged session's persisted outcome record (``session_outcomes``)."""

    model_config = _TOLERANT

    label: str = ""
    reason: str = ""
    goal: str | None = None
    turns: int = 0
    duration_s: int = 0
    models: list[str] = []
    errors: int = 0
    corrections: int = 0
    last_ts: float | None = None


class Verdict(BaseModel):
    """Normalized per-session verdict: a closed label plus a one-line reason."""

    model_config = _TOLERANT

    label: str
    reason: str


class Row(BaseModel):
    """One row of the 9-column corpus parquet contract (see ``corpus.py``)."""

    model_config = _TOLERANT

    external_id: str
    source: str
    content_hash: str
    title: str | None
    url: str | None
    host: str | None
    timestamp: int | None
    body: str
    meta_json: str


class State(BaseModel):
    """Per-(user, project) distillation state persisted as JSON."""

    model_config = _TOLERANT

    project: str | None = None
    items: list[Item] = []
    # session id -> fingerprint of the last-distilled revision of that session.
    distilled_sessions: dict[str, str] = {}
    session_outcomes: dict[str, SessionRecord] = {}
