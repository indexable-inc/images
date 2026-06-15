"""ix-distiller CLI: transcripts -> lessons + outcome verdicts -> corpus slices."""

from __future__ import annotations

import argparse
import getpass
import socket
import sys
from pathlib import Path

from . import corpus, distill, markdown, state, transcripts


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="ix-distiller",
        description=(
            "Distill ReasoningBank-style lessons from local Claude Code "
            "transcripts into facts markdown plus distilled_facts and "
            "session_outcomes corpus parquet slices."
        ),
    )
    parser.add_argument("--days", type=float, default=7.0, help="lookback window (default 7)")
    parser.add_argument("--user", default=getpass.getuser(), help="user the transcripts belong to")
    parser.add_argument(
        "--host", default=socket.gethostname().split(".")[0], help="host tag for the slice"
    )
    parser.add_argument(
        "--claude-root",
        type=Path,
        default=Path.home() / ".claude" / "projects",
        help="Claude Code projects dir (default ~/.claude/projects)",
    )
    parser.add_argument("--out", type=Path, default=Path("distilled"), help="output directory")
    parser.add_argument("--model", default=distill.DEFAULT_MODEL, help="distillation model id")
    parser.add_argument("--claude-bin", default="claude", help="claude binary for headless calls")
    parser.add_argument(
        "--max-sessions-per-project",
        type=int,
        default=20,
        help="cap of new sessions distilled per project per run",
    )
    parser.add_argument(
        "--max-new-items", type=int, default=8, help="cap of new items per project per run"
    )
    parser.add_argument(
        "--project",
        action="append",
        default=None,
        help="only distill projects whose path contains this substring (repeatable)",
    )
    parser.add_argument(
        "--min-sessions", type=int, default=1, help="skip projects with fewer new sessions"
    )
    parser.add_argument("--upload", action="store_true", help="upload the slice to MinIO")
    parser.add_argument("--endpoint", default="http://127.0.0.1:9010", help="S3 endpoint")
    parser.add_argument("--bucket", default="ix-history", help="bucket name")
    parser.add_argument("--prefix", default="corpus", help="key prefix inside the bucket")
    parser.add_argument(
        "--env-file", type=Path, default=None, help="EnvironmentFile with S3 credentials"
    )
    return parser


def run(args: argparse.Namespace) -> int:
    groups = transcripts.scan(args.claude_root, args.days)
    # Drop the distiller's own headless sessions (self-distillation guard).
    groups = {
        project: kept
        for project, sessions in groups.items()
        if (
            kept := [
                s
                for s in sessions
                if not (s.goal or "").startswith(distill.PROMPT_SENTINEL)
            ]
        )
    }
    if args.project:
        groups = {
            project: sessions
            for project, sessions in groups.items()
            if any(needle in project for needle in args.project)
        }
    if not groups:
        print(f"no sessions in the last {args.days:g} days under {args.claude_root}")
        return 0

    all_items_by_project: dict[str, list[dict]] = {}
    all_outcomes_by_project: dict[str, dict[str, dict]] = {}

    def keep_state(project: str, st: dict) -> None:
        if st["items"]:
            all_items_by_project[project] = st["items"]
        if st["session_outcomes"]:
            all_outcomes_by_project[project] = st["session_outcomes"]

    for project, sessions in sorted(groups.items()):
        slug = transcripts.project_slug(project)
        st = state.load(args.out, args.user, slug)
        st["project"] = project
        seen = st["distilled_sessions"]
        fresh = [
            s
            for s in sessions
            if seen.get(s.session_id) != s.fingerprint()
        ][: args.max_sessions_per_project]
        if len(fresh) < args.min_sessions:
            keep_state(project, st)
            print(f"[{slug}] no new sessions; keeping {len(st['items'])} items")
            continue

        digests = [transcripts.digest(s) for s in fresh]
        prompt = distill.build_prompt(project, st["items"], digests, max_new=args.max_new_items)
        print(
            f"[{slug}] distilling {len(fresh)} session(s) "
            f"({len(st['items'])} existing items) via {args.model} ..."
        )
        try:
            result = distill.run_claude(prompt, model=args.model, claude_bin=args.claude_bin)
        except Exception as error:  # noqa: BLE001 - one project must not sink the run
            print(f"[{slug}] distillation failed: {error}", file=sys.stderr)
            keep_state(project, st)
            continue

        sessions_meta = {s.session_id: {"last_ts": s.last_ts} for s in fresh}
        st["items"] = distill.apply_operations(
            st["items"], result.operations, sessions_meta, max_new=args.max_new_items
        )
        verdicts = distill.session_verdicts(result.session_outcomes, fresh)
        for s in fresh:
            verdict = verdicts[s.session_id]
            st["session_outcomes"][s.session_id] = {
                "label": verdict["label"],
                "reason": verdict["reason"],
                "goal": s.goal,
                "turns": s.message_count,
                "duration_s": (
                    int(s.last_ts - s.first_ts) if s.first_ts and s.last_ts else 0
                ),
                "models": s.models,
                "errors": len(s.errors),
                "corrections": len(s.corrections),
                "last_ts": s.last_ts,
            }
            seen[s.session_id] = s.fingerprint()
        state.save(args.out, args.user, slug, st)
        labels = ", ".join(f"{s.session_id}={verdicts[s.session_id]['label']}" for s in fresh)
        print(f"[{slug}] session outcomes: {labels}")
        if st["items"]:
            md_path = markdown.write(args.out, args.user, slug, project, st["items"])
            print(f"[{slug}] {len(st['items'])} items -> {md_path}")
        else:
            print(f"[{slug}] model found nothing worth keeping")
        keep_state(project, st)

    item_rows = [
        corpus.item_row(
            item,
            project,
            args.host,
            args.user,
            session_labels={
                sid: rec["label"]
                for sid, rec in all_outcomes_by_project.get(project, {}).items()
            },
        )
        for project, items in sorted(all_items_by_project.items())
        for item in items
    ]
    session_rows = [
        corpus.session_row(sid, rec, project, args.host, args.user)
        for project, recs in sorted(all_outcomes_by_project.items())
        for sid, rec in sorted(recs.items())
    ]
    if session_rows:
        counts: dict[str, int] = {}
        for project, recs in all_outcomes_by_project.items():
            for rec in recs.values():
                counts[rec["label"]] = counts.get(rec["label"], 0) + 1
        print(
            "outcome label distribution: "
            + ", ".join(f"{label}={n}" for label, n in sorted(counts.items()))
        )
    if not item_rows and not session_rows:
        print("no items or session outcomes at all; not writing slices")
        return 0

    for source, rows in ((corpus.SOURCE, item_rows), (corpus.SESSIONS_SOURCE, session_rows)):
        if not rows:
            continue
        rel = f"host={args.host}/user={args.user}/source={source}"
        slice_dir = args.out / "corpus" / Path(rel)
        corpus.write_slice(rows, slice_dir)
        count = corpus.validate_slice(slice_dir, source=source)
        print(f"slice OK: {count} rows in {slice_dir} (schema + hashes validated)")

        if args.upload:
            from . import upload

            key_prefix = f"{args.prefix.rstrip('/')}/{rel}"
            uploaded = upload.upload_slice(
                slice_dir, args.endpoint, args.bucket, key_prefix, env_file=args.env_file
            )
            for uri in uploaded:
                print(f"uploaded {uri}")
    return 0


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    return run(args)
