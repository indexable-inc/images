"""Distill Claude Code transcripts into ReasoningBank-style lessons.

The distiller reads local Claude Code transcripts (``~/.claude/projects``),
groups sessions by project, extracts success/failure/user-correction signals,
and asks a headless ``claude -p`` run to maintain an itemized set of
strategy-level lessons per (user, project). Items are merged incrementally
(stable ids, update-not-rewrite) per ACE's context-collapse warning -- the
model never regenerates the whole lesson set. The same call judges every
session it was shown (success / partial / failure / abandoned + one-line
reason), and lessons whose evidence includes a failed session carry that
label in their meta (failure-derived guardrails are the most valuable).

Outputs:
- human-readable markdown per (user, project) under ``<out>/facts/``
- corpus parquet slices conforming to the 9-column contract of
  ``packages/sink/parquet`` (external_id, source, content_hash, title, url,
  host, timestamp, body, meta_json) plus the ``_manifest.json`` corpus hash,
  at ``<out>/corpus/host=<h>/user=<u>/source=distilled_facts/`` (lessons) and
  ``.../source=session_outcomes/`` (per-session verdicts) so the leader fold
  ingests them with zero Rust changes (see audit: NEW DERIVED SOURCE IS
  CHEAP).
"""

__version__ = "0.1.0"
