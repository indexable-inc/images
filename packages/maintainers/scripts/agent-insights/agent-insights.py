#!/usr/bin/env python3
"""Graph how often you interrupt your local CLI coding agents over time.

Reads the on-disk session transcripts that Claude Code and Codex already write,
and renders a self-contained SVG (no dependencies) plus a text summary.

  Claude Code : ~/.claude/projects/**/<session>.jsonl
  Codex       : ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl

A "turn" is one bout of the agent running after you hand it work. An "interrupt"
is you cutting that bout off mid-flight.

  Codex   turn      = task_started -> task_complete | turn_aborted
          interrupt = turn_aborted with reason == "interrupted"   (exact ts)
  Claude  turn      = a human prompt -> last agent activity before your next input
          interrupt = a user message "[Request interrupted by user]..."
          (tool_result messages also arrive as role=user; they are NOT prompts)

The default graph is a weekly trend of interrupts per hour of agent run time,
split Claude vs Codex: "am I getting twitchier over time, and with which tool".

Two data quirks are handled so the numbers are real:
  - Codex replays the full parent transcript into every resumed/forked rollout
    file, re-stamping those events at the resume instant. Only "live" events
    (more than a few seconds after that file's session_meta timestamp) are
    counted, which dedupes resumes and drops the fake ~0s interrupt durations.
  - Claude session files are self-contained (verified: no cross-file message
    duplication), so they are counted as-is.

Usage:
  python3 agent-insights.py                      # writes agent-insights.svg, prints summary
  python3 agent-insights.py --out /tmp/i.svg
  python3 agent-insights.py --since 2026-01-01
  python3 agent-insights.py --tool codex
  python3 agent-insights.py --no-svg             # text summary only
  python3 agent-insights.py --claude-dir ~/.claude/projects --codex-dir ~/.codex/sessions
"""
from __future__ import annotations

import argparse
import json
from collections import defaultdict
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone, UTC
from pathlib import Path

INTERRUPT_MARK = "[Request interrupted by user"
# Codex re-stamps replayed (resumed/forked) events at the resume instant; a real
# turn lands seconds-to-hours later. Anything within this window of the file's
# session_meta timestamp is a replay and is skipped.
CODEX_REPLAY_GUARD_S = 3.0

CLAUDE_BLUE = "#2563eb"
CODEX_ORANGE = "#ea580c"


@dataclass
class Turn:
    tool: str           # "claude" | "codex"
    start: datetime     # when this bout began (UTC)
    run_seconds: float  # how long the agent ran in this bout
    interrupted: bool    # did you cut it off


def parse_ts(s: str | None) -> datetime | None:
    if not s:
        return None
    try:
        return datetime.fromisoformat(s).astimezone(UTC)
    except (ValueError, AttributeError):
        return None


# --- Claude ---------------------------------------------------------------

def claude_user_kind(msg: dict) -> str | None:  # type: ignore[type-arg]
    """Classify a role=user entry: interrupt | prompt | tool_result | None."""
    content = msg.get("content")
    parts = []
    if isinstance(content, str):
        parts.append(content)
    elif isinstance(content, list):
        for item in content:
            if not isinstance(item, dict):
                continue
            if item.get("type") == "tool_result":
                return "tool_result"
            if item.get("type") == "text":
                parts.append(item.get("text", ""))
    text = "\n".join(parts).strip()
    if not text:
        return None
    if text.startswith(INTERRUPT_MARK):
        return "interrupt"
    if text.startswith(("<command-", "<local-command")):
        return None
    return "prompt"


def scan_claude(root: str, since: datetime | None) -> list[Turn]:
    turns: list[Turn] = []
    for fp in Path(root).rglob("*.jsonl"):
        if "subagents" in fp.parts:  # the user interrupts the main agent, not sidechains
            continue
        run_start = None
        last_activity = None
        try:
            with fp.open(encoding="utf-8") as f:
                for raw in f:
                    stripped = raw.strip()
                    if not stripped:
                        continue
                    try:
                        d = json.loads(stripped)
                    except json.JSONDecodeError:
                        continue
                    if d.get("isSidechain"):
                        break
                    ts = parse_ts(d.get("timestamp"))
                    typ = d.get("type")
                    if typ == "user":
                        kind = claude_user_kind(d.get("message", {}))
                        if kind == "tool_result":
                            if ts:
                                last_activity = ts
                        elif kind == "interrupt":
                            if run_start and ts and (since is None or run_start >= since):
                                turns.append(Turn("claude", run_start,
                                                  (ts - run_start).total_seconds(), interrupted=True))
                            run_start = last_activity = None
                        elif kind == "prompt":
                            if run_start and last_activity and (since is None or run_start >= since):
                                turns.append(Turn("claude", run_start,
                                                  (last_activity - run_start).total_seconds(), interrupted=False))
                            run_start = last_activity = ts
                        elif ts:
                            last_activity = ts
                    elif typ == "assistant" and ts:
                        last_activity = ts
            if run_start and last_activity and (since is None or run_start >= since):
                turns.append(Turn("claude", run_start,
                                  (last_activity - run_start).total_seconds(), interrupted=False))
        except OSError:
            continue
    return turns


# --- Codex ----------------------------------------------------------------

def codex_meta_ts(fp: Path) -> datetime | None:
    try:
        with fp.open(encoding="utf-8") as f:
            d = json.loads(f.readline())
    except (OSError, json.JSONDecodeError):
        return None
    if d.get("type") != "session_meta":
        return None
    p = d.get("payload") if isinstance(d.get("payload"), dict) else {}
    return parse_ts(p.get("timestamp"))


def scan_codex(root: str, since: datetime | None) -> list[Turn]:
    turns: list[Turn] = []
    for fp in Path(root).glob("*/*/*/rollout-*.jsonl"):
        meta_ts = codex_meta_ts(fp)
        cur_start = None
        try:
            with fp.open(encoding="utf-8") as f:
                for raw in f:
                    stripped = raw.strip()
                    if not stripped:
                        continue
                    try:
                        d = json.loads(stripped)
                    except json.JSONDecodeError:
                        continue
                    if d.get("type") != "event_msg":
                        continue
                    p = d.get("payload") if isinstance(d.get("payload"), dict) else {}
                    ptype = p.get("type")
                    if ptype not in ("task_started", "task_complete", "turn_aborted"):
                        continue
                    ts = parse_ts(d.get("timestamp"))
                    live = (meta_ts is None or ts is None
                            or (ts - meta_ts).total_seconds() > CODEX_REPLAY_GUARD_S)
                    if not live:
                        if ptype != "task_started":
                            cur_start = None
                        continue
                    if ptype == "task_started":
                        cur_start = ts
                    elif ptype in ("task_complete", "turn_aborted"):
                        interrupted = ptype == "turn_aborted" and p.get("reason") == "interrupted"
                        if cur_start and ts and (since is None or cur_start >= since):
                            turns.append(Turn("codex", cur_start,
                                              (ts - cur_start).total_seconds(), interrupted))
                        cur_start = None
        except OSError:
            continue
    return turns


# --- metrics --------------------------------------------------------------

def week_start(dt: datetime) -> datetime:
    d = (dt - timedelta(days=dt.weekday())).date()
    return datetime(d.year, d.month, d.day, tzinfo=UTC)


def pct(vals: list[float], p: float) -> float:
    if not vals:
        return 0.0
    s = sorted(vals)
    return s[max(0, min(len(s) - 1, round(p / 100 * (len(s) - 1))))]


def fmt_dur(seconds: float) -> str:
    secs = int(seconds)
    if secs < 90:
        return f"{secs}s"
    if secs < 5400:
        return f"{secs / 60:.1f}m"
    return f"{secs / 3600:.1f}h"


def summarize(name: str, turns: list[Turn]) -> None:
    n = len(turns)
    interrupts = sum(1 for t in turns if t.interrupted)
    run = sum(t.run_seconds for t in turns)
    ttis = [t.run_seconds for t in turns if t.interrupted]
    hours = run / 3600 or 1e-9
    print(f"\n=== {name} ===")
    print(f"  agent turns           : {n}")
    print(f"  interrupts            : {interrupts}")
    print(f"  total agent run time  : {fmt_dur(run)}")
    print(f"  interrupts / hour run : {interrupts / hours:.2f}")
    print(f"  interrupt rate / turn : {(interrupts / n * 100) if n else 0:.1f}%")
    if ttis:
        print(f"  time-to-interrupt     : median {fmt_dur(pct(ttis, 50))}, "
              f"p90 {fmt_dur(pct(ttis, 90))}, max {fmt_dur(max(ttis))}")


def weekly_series(
    turns: list[Turn],
    min_week_hours: float,
) -> tuple[list[datetime], dict[str, list[tuple[datetime, float | None]]]]:
    """Return {tool: [(week_start, interrupts_per_hour), ...]} over a shared week axis."""
    agg = defaultdict(lambda: defaultdict(lambda: [0, 0.0]))  # tool -> week -> [intr, run_s]
    weeks = set()
    for t in turns:
        w = week_start(t.start)
        weeks.add(w)
        cell = agg[t.tool][w]
        cell[0] += 1 if t.interrupted else 0
        cell[1] += t.run_seconds
    axis = sorted(weeks)
    series = {}
    for tool, byweek in agg.items():
        pts = []
        for w in axis:
            intr, run_s = byweek.get(w, [0, 0.0])
            hrs = run_s / 3600
            pts.append((w, intr / hrs if hrs >= min_week_hours else None))
        series[tool] = pts
    return axis, series


# --- SVG rendering --------------------------------------------------------

def render_svg(
    axis: list[datetime],
    series: dict[str, list[tuple[datetime, float | None]]],
    out_path: str,
) -> None:
    W, H = 940, 460
    ml, mr, mt, mb = 64, 150, 48, 70
    pw, ph = W - ml - mr, H - mt - mb
    ymax = max([v for pts in series.values() for _, v in pts if v is not None] + [1.0])
    ymax = max(1.0, ymax * 1.15)
    n = max(1, len(axis))

    def x(i: int) -> float:
        return ml + (pw * i / (n - 1) if n > 1 else pw / 2)

    def y(v: float) -> float:
        return mt + ph * (1 - v / ymax)

    colors = {"claude": CLAUDE_BLUE, "codex": CODEX_ORANGE}
    labels = {"claude": "Claude Code", "codex": "Codex"}
    s = []
    s.append(f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" '
             f'viewBox="0 0 {W} {H}" font-family="Inter, system-ui, sans-serif">')
    s.append(f'<rect width="{W}" height="{H}" fill="#ffffff"/>')
    s.append(f'<text x="{ml}" y="26" font-size="17" font-weight="600" fill="#111827">'
             f'Agent interrupts per hour of run time, by week</text>')

    # y gridlines + labels
    for k in range(6):
        v = ymax * k / 5
        yy = y(v)
        s.append(f'<line x1="{ml}" y1="{yy:.1f}" x2="{ml + pw}" y2="{yy:.1f}" '
                 f'stroke="#e5e7eb" stroke-width="1"/>')
        s.append(f'<text x="{ml - 10}" y="{yy + 4:.1f}" font-size="11" fill="#6b7280" '
                 f'text-anchor="end">{v:.1f}</text>')
    s.append(f'<text x="18" y="{mt + ph / 2:.0f}" font-size="12" fill="#374151" '
             f'transform="rotate(-90 18 {mt + ph / 2:.0f})" text-anchor="middle">'
             f'interrupts / hour running</text>')

    # x labels (sparse: ~10 ticks)
    step = max(1, n // 10)
    for i, w in enumerate(axis):
        if i % step and i != n - 1:
            continue
        xx = x(i)
        s.append(f'<line x1="{xx:.1f}" y1="{mt + ph}" x2="{xx:.1f}" y2="{mt + ph + 5}" '
                 f'stroke="#9ca3af" stroke-width="1"/>')
        s.append(f'<text x="{xx:.1f}" y="{mt + ph + 20}" font-size="10" fill="#6b7280" '
                 f'text-anchor="middle" transform="rotate(35 {xx:.1f} {mt + ph + 20})">'
                 f'{w.strftime("%Y-%m-%d")}</text>')

    # lines (break across weeks with no data)
    for tool, pts in series.items():
        col = colors.get(tool, "#16a34a")
        run = []
        for i, (_w, v) in enumerate(pts):
            if v is None:
                if len(run) > 1:
                    s.append(f'<polyline fill="none" stroke="{col}" stroke-width="2.5" '
                             f'points="{" ".join(run)}"/>')
                run = []
                continue
            run.append(f"{x(i):.1f},{y(v):.1f}")
        if len(run) > 1:
            s.append(f'<polyline fill="none" stroke="{col}" stroke-width="2.5" '
                     f'points="{" ".join(run)}"/>')
        for i, (_w, v) in enumerate(pts):
            if v is not None:
                s.append(f'<circle cx="{x(i):.1f}" cy="{y(v):.1f}" r="3" fill="{col}"/>')

    # legend
    lx, ly = ml + pw + 24, mt + 8
    for tool in series:
        col = colors.get(tool, "#16a34a")
        s.append(f'<rect x="{lx}" y="{ly - 9}" width="14" height="14" rx="3" fill="{col}"/>')
        s.append(f'<text x="{lx + 20}" y="{ly + 2}" font-size="12" fill="#374151">'
                 f'{labels.get(tool, tool)}</text>')
        ly += 24

    s.append('</svg>')
    Path(out_path).write_text("\n".join(s), encoding="utf-8")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--claude-dir", default=str(Path("~/.claude/projects").expanduser()))
    ap.add_argument("--codex-dir", default=str(Path("~/.codex/sessions").expanduser()))
    ap.add_argument("--tool", choices=["claude", "codex", "both"], default="both")
    ap.add_argument("--since", help="ISO date, e.g. 2026-01-01 (only later activity)")
    ap.add_argument("--out", default="agent-insights.svg", help="SVG output path")
    ap.add_argument("--no-svg", action="store_true", help="text summary only")
    ap.add_argument("--min-week-hours", type=float, default=0.5,
                    help="drop weekly points with less than this many run-hours (noise)")
    args = ap.parse_args()

    since = parse_ts(args.since + "T00:00:00+00:00") if args.since else None

    turns: list[Turn] = []
    if args.tool in ("claude", "both") and Path(args.claude_dir).is_dir():
        turns += scan_claude(args.claude_dir, since)
    if args.tool in ("codex", "both") and Path(args.codex_dir).is_dir():
        turns += scan_codex(args.codex_dir, since)

    if not turns:
        raise SystemExit("no agent turns found; check --claude-dir / --codex-dir")

    if args.tool in ("claude", "both"):
        summarize("CLAUDE CODE", [t for t in turns if t.tool == "claude"])
    if args.tool in ("codex", "both"):
        summarize("CODEX", [t for t in turns if t.tool == "codex"])
    if args.tool == "both":
        summarize("COMBINED", turns)

    if not args.no_svg:
        axis, series = weekly_series(turns, args.min_week_hours)
        render_svg(axis, series, args.out)
        print(f"\nwrote {args.out}  ({len(axis)} weeks, {len(turns)} turns)")


if __name__ == "__main__":
    main()
