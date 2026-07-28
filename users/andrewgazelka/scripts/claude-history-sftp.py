#!/usr/bin/env python3
"""Query Claude Code history across remote hosts, read over SFTP with polars-sftp.

For each given SSH host it finds every Claude history file
(`~/.claude/history.jsonl` plus the session transcripts under
`~/.claude/projects/**/*.jsonl`; the `~/.claude/jobs/**` artifacts are skipped),
reads each over SFTP, and aggregates them with Polars: records per host, record
types across the transcripts, the biggest sessions, and (where a `history.jsonl`
prompt log exists) the busiest projects and most recent prompts.

This dogfoods `packages/polars-sftp`: transcripts are read eagerly with
`read_sftp` (one fetch each), and the prompt log is queried lazily through the
`scan_sftp` IO plugin (column projection pushed into the reader).

Setup (needs Python Polars 1.40.x to match the wheel's pinned ABI):

    nix build .#polars-sftp
    python -m venv .venv
    .venv/bin/pip install 'polars==1.40.*' result/*.whl
    .venv/bin/python users/andrewgazelka/scripts/claude-history-sftp.py hil-compute-1 hil-compute-2

Hosts are SSH aliases resolved with `ssh -G`, so your `~/.ssh/config`
host/port/user/identity settings apply. `--limit N` caps files per host (handy
for a quick look); without it, every history file is read.
"""

from __future__ import annotations

import shlex
import subprocess
import sys
from collections import Counter
from pathlib import Path

import polars as pl
from polars_sftp import read_sftp, scan_sftp

DEFAULT_HOSTS = ["hil-compute-1", "hil-compute-2", "hil-compute-3"]


def resolve(alias: str) -> dict[str, str]:
    """Resolve an SSH alias to connection params with `ssh -G` (reads ssh_config)."""
    out = subprocess.run(
        ["ssh", "-G", alias], capture_output=True, text=True, check=True
    ).stdout
    cfg: dict[str, str] = {}
    for line in out.splitlines():
        key, _, val = line.partition(" ")
        # `ssh -G` lists identityfile multiple times; keep the first of each key.
        if key in ("hostname", "port", "user", "identityfile") and key not in cfg:
            cfg[key] = val
    return cfg


def connection(alias: str) -> tuple[str, dict[str, object]]:
    """`(hostname, polars-sftp kwargs)` for `alias`."""
    cfg = resolve(alias)
    key = cfg.get("identityfile")
    key_path = Path(key).expanduser() if key else None
    conn: dict[str, object] = {
        "port": int(cfg.get("port", "22")),
        "username": cfg.get("user"),
        # `ssh -G` reports default identity paths even when the file is absent;
        # only pass one that exists, else leave it None so polars-sftp falls back
        # to the SSH agent.
        "private_key": key_path if (key_path and key_path.exists()) else None,
        "storage_format": "ndjson",
    }
    return cfg["hostname"], conn


def history_files(alias: str) -> list[str]:
    """The host's real Claude history files: `history.jsonl` + project transcripts.

    Deliberately excludes `~/.claude/jobs/**` (background-job timelines and event
    artifacts), which are not conversation history.
    """
    remote = (
        'ls -1 "$HOME"/.claude/history.jsonl 2>/dev/null; '
        'find "$HOME"/.claude/projects -type f -name "*.jsonl" 2>/dev/null'
    )
    # Pass the whole pipeline as one argument; ssh concatenates extra argv with
    # spaces and the remote shell re-splits, which would mangle `bash -lc <cmd>`.
    out = subprocess.run(
        ["ssh", alias, "bash -lc " + shlex.quote(remote)],
        capture_output=True,
        text=True,
        check=False,
    ).stdout
    return [p for p in out.splitlines() if p.strip()]


def main(hosts: list[str], limit: int | None) -> None:
    per_host: dict[str, int] = {}
    types: Counter[str] = Counter()
    sessions: list[dict[str, object]] = []
    prompt_frames: list[pl.DataFrame] = []

    for alias in hosts:
        try:
            hostname, conn = connection(alias)
        except subprocess.CalledProcessError as exc:
            print(f"{alias}: cannot resolve ({exc})", file=sys.stderr)
            continue

        files = history_files(alias)
        if limit is not None:
            files = files[:limit]
        records = unreadable = 0

        for path in files:
            if path.endswith("history.jsonl"):
                # Showcase the lazy IO plugin (projection pushed into the read),
                # collected per file so one unreadable history.jsonl is skipped,
                # not fatal to the whole multi-host run.
                try:
                    prompt_frames.append(
                        scan_sftp(hostname, path, **conn)
                        .select("display", "project", "timestamp")
                        .with_columns(host=pl.lit(alias))
                        .collect()
                    )
                except Exception as exc:
                    unreadable += 1
                    print(f"  ! {alias}:{Path(path).name}: {exc}", file=sys.stderr)
                continue
            try:
                df = read_sftp(hostname, path, **conn)  # one fetch, eager
            except Exception as exc:  # a transcript polars can't infer; skip + report
                unreadable += 1
                print(f"  ! {alias}:{Path(path).name}: {exc}", file=sys.stderr)
                continue
            records += df.height
            if "type" in df.columns:
                types.update(
                    df.get_column("type").drop_nulls().cast(pl.Utf8).to_list()
                )
            sessions.append(
                {
                    "host": alias,
                    "session": Path(path).name.removesuffix(".jsonl"),
                    "records": df.height,
                }
            )

        per_host[alias] = records
        print(f"{alias}: {len(files)} files, {records} transcript records ({unreadable} unreadable)")

    if sessions:
        print("\n== records by transcript type ==")
        for kind, count in types.most_common(12):
            print(f"  {count:>8}  {kind}")
        print("\n== biggest sessions ==")
        with pl.Config(tbl_rows=10):
            print(pl.DataFrame(sessions).sort("records", descending=True).head(10))

    if prompt_frames:
        hist = pl.concat(prompt_frames, how="diagonal_relaxed")
        print(f"\n== history.jsonl: {hist.height} prompts across {hist['host'].n_unique()} host(s) ==")
        print("\nbusiest projects:")
        print(hist.group_by("project").len().sort("len", descending=True).head(10))
        print("\nmost recent prompts:")
        with pl.Config(fmt_str_lengths=60, tbl_rows=10):
            print(
                hist.sort("timestamp", descending=True)
                .select("host", "project", "display")
                .head(10)
            )
    elif not sessions:
        print("\nno Claude history files found on any host")


def parse_args(argv: list[str]) -> tuple[list[str], int | None]:
    hosts: list[str] = []
    limit: int | None = None
    it = iter(argv)
    for arg in it:
        raw: str | None = None
        if arg == "--limit":
            raw = next(it, None)
            if raw is None:
                raise SystemExit("--limit needs a number")
        elif arg.startswith("--limit="):
            raw = arg.split("=", 1)[1]
        else:
            hosts.append(arg)
            continue
        try:
            limit = int(raw)
        except ValueError:
            raise SystemExit(f"--limit must be an integer, got {raw!r}") from None
        if limit < 0:
            raise SystemExit("--limit must be >= 0")
    return hosts or DEFAULT_HOSTS, limit


if __name__ == "__main__":
    main(*parse_args(sys.argv[1:]))
