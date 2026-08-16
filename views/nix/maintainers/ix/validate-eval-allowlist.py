#!/usr/bin/env python3
"""Validate eval-allowlist.toml, and print the ids it accepts.

An allowlist entry is a standing waiver: it says the two evaluators may
disagree on one corpus case and the differ must not fail. lang-diff.sh used
to enforce it with `grep -q '^id = "<name>"'`, which reads exactly one field.
`tier`, `reason` and `approved` were never parsed by anything, so an entry
could carry any tier it liked, no reason at all, or an approval from a
machine, and still silence a real divergence.

The rule this exists to enforce is the two-tier parity bar (CLAUDE.md,
"Parity bar"). Tier 2 -- presentation, error wording, trace shape -- is a
judgement an agent can make and defend in `reason`, because "did both arms
do the same thing" is checkable. A `semantic-divergence` is not tier 2: it
says the evaluators may disagree about what a program MEANS, which is a
decision about the language, so it needs a human named in
eval-allowlist-approvers.txt -- a checked-in list, and therefore a reviewed
diff.

Tier 1 (`.drv` bytes, outPaths, drvPaths, anything feeding a hash) has no
representation here at all, deliberately. There is no accepted byte-level
divergence, so there is no field that could express one, and the gates that
compare hashes do not read this file.

    validate-eval-allowlist.py ALLOWLIST APPROVERS [--ids]

Exit 0 iff every entry is well formed. `--ids` additionally writes the
accepted ids to stdout, one per line, which is how lang-diff.sh gets the set
it matches against.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - python < 3.11
    print(
        "validate-eval-allowlist: need python 3.11+ for tomllib "
        f"(running {sys.version.split()[0]})",
        file=sys.stderr,
    )
    raise SystemExit(2) from None

# Kept in step with eval-allowlist.toml's header, which is the prose version
# of this. A tier outside these is a typo or an invention, and either way
# nothing downstream knows what it means.
#
# Tier 2: the arms do the same thing and say it differently. An agent may
# approve one, so `approved` is required but unconstrained.
PRESENTATION_TIERS = frozenset({"presentation-divergence", "error-text", "trace-format"})

# Not tier 2: the arms disagree about meaning. Needs a human.
SEMANTIC_TIERS = frozenset({"semantic-divergence"})

TIERS = PRESENTATION_TIERS | SEMANTIC_TIERS

REQUIRED = ("id", "tier", "reason", "approved")

# A waiver whose reason is "flaky" or "known" is not a triage note, it is a
# shrug. Tier 2 approval rests entirely on the reason being checkable, so the
# one thing enforced mechanically is that somebody wrote a sentence.
MIN_REASON_CHARS = 40

# `approved` is conventionally "<date> <name>[ <free text>]". Names are
# matched as whole words anywhere in the field rather than by position, so a
# trailing note ("; standing human gate: PR review") cannot hide the name and
# a substring ("claude" inside "claudia") cannot fake one.
WORD = re.compile(r"[A-Za-z0-9_.@-]+")


def read_approvers(path: Path) -> frozenset[str]:
    names: set[str] = set()
    for raw in path.read_text().splitlines():
        line = raw.split("#", 1)[0].strip()
        if line:
            names.add(line)
    return frozenset(names)


def validate(allowlist: Path, approvers_file: Path) -> tuple[list[str], list[str]]:
    """Return (accepted ids, errors). Errors empty means the file is valid."""
    errors: list[str] = []
    approvers = read_approvers(approvers_file)
    if not approvers:
        # An empty approver list would silently reject every semantic entry,
        # which looks like a strict gate and is really a missing file.
        errors.append(f"{approvers_file}: no approver names; refusing to judge approvals")

    with allowlist.open("rb") as fh:
        try:
            doc = tomllib.load(fh)
        except tomllib.TOMLDecodeError as exc:
            return [], [f"{allowlist}: not valid TOML: {exc}"]

    unknown_tables = set(doc) - {"divergence"}
    if unknown_tables:
        errors.append(f"{allowlist}: unknown top-level keys {sorted(unknown_tables)}")

    entries = doc.get("divergence", [])
    if not isinstance(entries, list):
        return [], [f"{allowlist}: [[divergence]] must be an array of tables"]

    ids: list[str] = []
    for n, entry in enumerate(entries, start=1):
        where = f"{allowlist}: [[divergence]] #{n}"
        missing = [k for k in REQUIRED if not str(entry.get(k, "")).strip()]
        if missing:
            errors.append(f"{where}: missing or empty {', '.join(missing)}")
            continue
        where = f"{allowlist}: {entry['id']}"
        extra = set(entry) - set(REQUIRED)
        if extra:
            errors.append(f"{where}: unknown fields {sorted(extra)}")
        if entry["tier"] not in TIERS:
            errors.append(f"{where}: tier '{entry['tier']}' is not one of {sorted(TIERS)}")
        if entry["id"] in ids:
            errors.append(f"{where}: duplicate id")
        if len(entry["reason"].strip()) < MIN_REASON_CHARS:
            errors.append(
                f"{where}: reason is {len(entry['reason'].strip())} characters, "
                f"want at least {MIN_REASON_CHARS}. Every tier here is accepted on "
                "the strength of its reason; say what was compared and why the "
                "difference cannot reach a user's build."
            )
        if entry["tier"] in SEMANTIC_TIERS:
            words = set(WORD.findall(entry["approved"]))
            if not (words & approvers):
                errors.append(
                    f"{where}: tier is {entry['tier']}, which is not a presentation "
                    f"divergence, but approved='{entry['approved']}' names nobody in "
                    f"{approvers_file.name} ({', '.join(sorted(approvers))}). Saying "
                    "the two evaluators may disagree about what a program MEANS is a "
                    "decision about the language, so it needs a human: add the "
                    "reviewer to that file in the same commit. If the arms actually "
                    "agree and only the wording differs, retier it as one of: "
                    f"{', '.join(sorted(PRESENTATION_TIERS))}."
                )
        ids.append(entry["id"])

    return ids, errors


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("allowlist", type=Path)
    ap.add_argument("approvers", type=Path)
    ap.add_argument("--ids", action="store_true", help="print accepted ids on stdout")
    args = ap.parse_args()

    for p in (args.allowlist, args.approvers):
        if not p.is_file():
            print(f"validate-eval-allowlist: no such file: {p}", file=sys.stderr)
            return 2

    ids, errors = validate(args.allowlist, args.approvers)
    for e in errors:
        print(f"validate-eval-allowlist: {e}", file=sys.stderr)
    if errors:
        return 1
    if args.ids:
        print("\n".join(ids))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
