"""Per-(user, project) distillation state: items, session verdicts, seen set.

State lives under ``<out>/state/<user>/<project_slug>.json`` and is what makes
the run incremental: items carry stable ids across runs, and a session is
re-distilled only when its fingerprint (message count + last timestamp)
changed since the previous run.
"""

from __future__ import annotations

import json
from pathlib import Path

from pydantic import ValidationError

from .types import State


def state_path(out_dir: Path, user: str, slug: str) -> Path:
    return out_dir / "state" / user / f"{slug}.json"


def load(out_dir: Path, user: str, slug: str, legacy_slugs: list[str] | None = None) -> State:
    path = state_path(out_dir, user, slug)
    if not path.is_file():
        # First run after repo-keying: the canonical path does not exist yet, so
        # adopt the newest legacy per-cwd file (``home-u-repo.json``) if one is
        # present. The run then re-saves under the canonical slug, migrating the
        # learned items forward instead of starting from an empty corpus.
        for legacy in legacy_slugs or ():
            if legacy == slug:
                continue
            legacy_path = state_path(out_dir, user, legacy)
            if legacy_path.is_file():
                path = legacy_path
                break
        else:
            return State()
    try:
        data = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return State()
    if not isinstance(data, dict):
        return State()
    try:
        # The file is external (a prior run's output, possibly hand-edited); the
        # model fills any keys an older schema omitted (defaults) and drops keys a
        # newer schema added (extra="ignore") before the State shape is trusted.
        return State.model_validate(data)
    except ValidationError:
        # A legacy or hand-edited file that is valid JSON but fails schema
        # validation (e.g. an items[] entry with an unrecognised type that
        # cannot coerce) degrades to a fresh empty state rather than crashing,
        # matching the "corrupt file → empty state" contract above.
        return State()


def save(out_dir: Path, user: str, slug: str, state: State) -> Path:
    path = state_path(out_dir, user, slug)
    path.parent.mkdir(parents=True, exist_ok=True)
    # Match the historical on-disk shape exactly: items omit their unset
    # provenance stamps (the old total=False Item left them out), while every
    # other field -- including null project/goal/last_ts -- is written. So dump
    # the items with exclude_none and assemble the rest verbatim, then emit in
    # the same json.dumps(indent=1, sort_keys=True) style as before.
    payload: dict[str, object] = {
        "project": state.project,
        "items": [item.model_dump(mode="json", exclude_none=True) for item in state.items],
        "distilled_sessions": state.distilled_sessions,
        "session_outcomes": {
            sid: rec.model_dump(mode="json") for sid, rec in state.session_outcomes.items()
        },
    }
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(payload, indent=1, sort_keys=True))
    tmp.replace(path)  # atomic like the indexer's cursor write
    return path
