"""Render an eval run as a single self-contained, navigable HTML scorecard.

Inline CSS + a little inline JS, no external assets, dark-mode aware. The page
has three layers you can drill into:

1. a summary band: one card per eval with its headline score, streak, and cost;
2. per eval, a behaviors panel (name + full rubric + pass-rate + per-rollout
   chips) and a list of rollouts;
3. per rollout, the verdicts with the judge's evidence AND the full action
   timeline: every assistant message, thinking block, tool call with its input,
   tool result, and the final answer.

The same JSON that feeds this (``--json-out``) is the machine-readable raw data.
"""

from __future__ import annotations

import html
from collections.abc import Sequence

from .core import EvalReport


def _esc(value: object) -> str:
    return html.escape(str(value))


def _pct(value: float) -> str:
    return f"{value:.0%}"


def _num(value: object) -> float:
    return float(value) if isinstance(value, (int, float)) else 0.0


def _chip(label: str, value: str) -> str:
    return f'<span class="chip"><b>{_esc(label)}</b> {_esc(value)}</span>'


def _cost_chips(summary: dict[str, object]) -> str:
    cost = summary.get("cost")
    if not isinstance(cost, dict):
        return ""
    out: list[str] = []
    if "mean_duration_s" in cost:
        out.append(_chip("mean", f"{_num(cost['mean_duration_s']):.0f}s"))
    if "total_output_tokens" in cost:
        out.append(_chip("out tok", f"{int(_num(cost['total_output_tokens'])):,}"))
    if "total_input_tokens" in cost:
        out.append(_chip("in tok", f"{int(_num(cost['total_input_tokens'])):,}"))
    if "total_cost_usd" in cost:
        out.append(_chip("cost", f"${_num(cost['total_cost_usd']):.2f}"))
    return "".join(out)


# ---- step timeline ---------------------------------------------------------


def _step(step: dict[str, object]) -> str:
    kind = str(step.get("kind", ""))
    if kind == "text":
        return f'<div class="step text"><div class="lbl">assistant</div><div class="prose">{_esc(step.get("text", ""))}</div></div>'
    if kind == "thinking":
        return (
            '<details class="step thinking"><summary>thinking</summary>'
            f'<pre>{_esc(step.get("text", ""))}</pre></details>'
        )
    if kind == "tool_use":
        name = _esc(step.get("name", "?"))
        return (
            f'<details class="step tooluse" open><summary>🔧 <b>{name}</b></summary>'
            f'<pre>{_esc(step.get("input", ""))}</pre></details>'
        )
    if kind == "tool_result":
        err = " error" if step.get("is_error") else ""
        label = "tool result (error)" if step.get("is_error") else "tool result"
        return (
            f'<details class="step toolresult{err}"><summary>{label}</summary>'
            f'<pre>{_esc(step.get("text", ""))}</pre></details>'
        )
    if kind == "final":
        return f'<div class="step final"><div class="lbl">final answer</div><div class="prose">{_esc(step.get("text", ""))}</div></div>'
    return ""


def _timeline(case: dict[str, object]) -> str:
    steps = case.get("steps")
    if not isinstance(steps, list) or not steps:
        # No structured steps (e.g. a safe-mode run captured before this feature):
        # fall back to the compact transcript so nothing is lost.
        tr = case.get("transcript")
        if isinstance(tr, str) and tr.strip():
            return f'<pre class="fallback">{_esc(tr)}</pre>'
        return '<p class="muted">no transcript captured</p>'
    rows = "".join(_step(s) for s in steps if isinstance(s, dict))
    n = len(steps)
    return f'<div class="timeline"><div class="muted">{n} steps</div>{rows}</div>'


# ---- verdicts --------------------------------------------------------------


def _verdicts(case: dict[str, object]) -> str:
    present = case.get("present")
    evidence = case.get("evidence")
    if isinstance(present, dict):  # behaviors eval
        ev = evidence if isinstance(evidence, dict) else {}
        rows = "".join(
            f'<tr class="{"y" if v else "n"}"><td>{"✓" if v else "✗"}</td>'
            f"<td>{_esc(b)}</td><td>{_esc(ev.get(b, ''))}</td></tr>"
            for b, v in present.items()
        )
        return f'<table class="verdicts"><tr><th></th><th>behavior</th><th>evidence</th></tr>{rows}</table>'
    # first-principles / reverse-engineering: a single judged dimension
    if "verdict" in case:
        v = _esc(case.get("verdict", "?"))
        return (
            f'<table class="verdicts"><tr><td><b>verdict</b></td><td>{v}</td></tr>'
            f'<tr><td><b>answer</b></td><td>{_esc(case.get("answer", ""))}</td></tr>'
            f'<tr><td><b>evidence</b></td><td>{_esc(case.get("evidence", ""))}</td></tr></table>'
        )
    if "reverse_engineered" in case:
        re = bool(case.get("reverse_engineered"))
        return (
            f'<table class="verdicts"><tr><td><b>reverse-engineered</b></td>'
            f'<td>{"yes" if re else "no"}</td></tr>'
            f'<tr><td><b>answer</b></td><td>{_esc(case.get("answer", ""))}</td></tr>'
            f'<tr><td><b>evidence</b></td><td>{_esc(case.get("evidence", ""))}</td></tr></table>'
        )
    return ""


def _status(case: dict[str, object]) -> tuple[str, str]:
    if case.get("error"):
        return "bad", "ERROR"
    present = case.get("present")
    if isinstance(present, dict):
        ok = sum(1 for v in present.values() if v is True)
        cls = "good" if ok == len(present) else ("warn" if ok else "bad")
        return cls, f"{ok}/{len(present)}"
    if "verdict" in case:
        v = str(case.get("verdict", "?"))
        return {"validated": "good", "stale": "bad"}.get(v, "warn"), v
    if "reverse_engineered" in case:
        re = bool(case.get("reverse_engineered"))
        return ("good" if re else "bad"), ("RE" if re else "guessed")
    return "warn", "?"


def _rollout(case: dict[str, object], anchor: str) -> str:
    cls, badge = _status(case)
    cid = _esc(case.get("case_id", "?"))
    roll = _esc(case.get("rollout", ""))
    dur = case.get("duration_ms")
    dur_s = f"{_num(dur) / 1000:.0f}s" if isinstance(dur, (int, float)) and dur else "-"
    out_tok = int(_num(case.get("output_tokens")))
    cost = _num(case.get("cost_usd"))
    meta = f'<span class="rmeta">{dur_s} · {out_tok:,} tok · ${cost:.2f}</span>'
    summary_line = (
        f'<summary id="{anchor}"><span class="badge {cls}">{_esc(badge)}</span> '
        f"<b>{cid}</b> #{roll} {meta}</summary>"
    )
    body = _verdicts(case)
    err = case.get("error")
    if err:
        body += f'<p class="bad">{_esc(err)}</p>'
    body += '<div class="tl-h">action timeline</div>' + _timeline(case)
    return f'<details class="rollout">{summary_line}{body}</details>'


# ---- behaviors panel -------------------------------------------------------


def _behaviors_panel(summary: dict[str, object], cases: Sequence[dict[str, object]], eid: str) -> str:
    defs = summary.get("behavior_defs")
    rates = summary.get("per_behavior")
    if not isinstance(defs, list) or not defs:
        return ""
    rate_map = rates if isinstance(rates, dict) else {}
    blocks: list[str] = []
    for d in defs:
        if not isinstance(d, dict):
            continue
        bid = str(d.get("id", ""))
        rate = _num(rate_map.get(bid))
        # per-rollout chips that jump to the rollout
        chips: list[str] = []
        for i, c in enumerate(cases):
            present = c.get("present")
            if not (isinstance(present, dict) and bid in present):
                continue
            ok = present[bid] is True
            chips.append(
                f'<a class="dot {"y" if ok else "n"}" href="#{eid}-{i}" '
                f'title="{_esc(c.get("case_id", ""))} #{_esc(c.get("rollout", ""))}">'
                f'{"✓" if ok else "✗"}</a>'
            )
        blocks.append(
            f'<div class="beh"><div class="beh-head"><b>{_esc(d.get("name", bid))}</b>'
            f'<span class="beh-rate">{_pct(rate)}</span></div>'
            f'<div class="bar"><span style="width:{rate * 100:.0f}%"></span></div>'
            f'<div class="rubric">{_esc(d.get("rubric", ""))}</div>'
            f'<div class="dots">{"".join(chips)}</div></div>'
        )
    return f'<div class="panel">{"".join(blocks)}</div>'


# ---- sections --------------------------------------------------------------


def _eval_section(rep: EvalReport, idx: int) -> str:
    eid = f"e{idx}"
    streak = "" if rep.longest_streak is None else _chip("streak", str(rep.longest_streak))
    s = rep.summary
    counts = ""
    if "scored" in s or "rollouts" in s:
        total = int(_num(s.get("total", s.get("rollouts", 0))))
        errd = int(_num(s.get("errored", 0)))
        counts = _chip("runs", f"{total}" + (f" ({errd} err)" if errd else ""))
    head = (
        f'<h2 id="{eid}">{_esc(rep.name)} <span class="headline {_grade(rep.headline)}">{_pct(rep.headline)}</span></h2>'
        f'<div class="chips">{streak}{counts}{_cost_chips(s)}</div>'
    )
    panel = _behaviors_panel(s, rep.cases, eid)
    rollouts = "".join(_rollout(c, f"{eid}-{i}") for i, c in enumerate(rep.cases))
    return f'<section>{head}{panel}<h3>rollouts</h3>{rollouts}</section>'


def _grade(h: float) -> str:
    return "good" if h >= 0.8 else ("warn" if h >= 0.5 else "bad")


def _summary_card(rep: EvalReport, idx: int) -> str:
    return (
        f'<a class="scard" href="#e{idx}"><div class="scard-h">{_esc(rep.name)}</div>'
        f'<div class="scard-n {_grade(rep.headline)}">{_pct(rep.headline)}</div></a>'
    )


def render_html(metadata: dict[str, object], reports: Sequence[EvalReport]) -> str:
    meta_chips = "".join(_chip(k, str(v)) for k, v in metadata.items())
    cards = "".join(_summary_card(r, i) for i, r in enumerate(reports))
    sections = "".join(_eval_section(r, i) for i, r in enumerate(reports))
    return f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>system-prompt eval scorecard</title>
<style>
:root {{ color-scheme: light dark; --line:#8884; --soft:#8881; --good:#2e9d4e; --warn:#c98a00; --bad:#d4493f; }}
* {{ box-sizing: border-box; }}
body {{ font: 14px/1.55 ui-monospace,"Berkeley Mono",Menlo,monospace; margin: 0 auto; max-width: 68rem; padding: 1.5rem 1rem 6rem; }}
h1 {{ font-size: 1.4rem; margin: 0 0 .3rem; }}
h2 {{ font-size: 1.15rem; margin: 2rem 0 .2rem; }}
h3 {{ font-size: .95rem; opacity: .7; margin: 1.2rem 0 .4rem; text-transform: uppercase; letter-spacing: .04em; }}
.muted {{ opacity: .6; }}
.good {{ color: var(--good); }} .warn {{ color: var(--warn); }} .bad {{ color: var(--bad); }}
.headline {{ float: right; font-size: 1.5rem; font-weight: 800; }}
.chips {{ margin: .3rem 0 .6rem; }}
.chip {{ display:inline-block; background: var(--soft); border-radius: 6px; padding: 2px 8px; margin: 2px 3px 2px 0; font-size: 12px; }}
.cards {{ display:flex; flex-wrap:wrap; gap:.6rem; margin:.8rem 0 1.2rem; }}
.scard {{ flex:1 1 12rem; text-decoration:none; color:inherit; border:1px solid var(--line); border-radius:10px; padding:.7rem .9rem; }}
.scard-h {{ font-size:.85rem; opacity:.8; }}
.scard-n {{ font-size:1.7rem; font-weight:800; }}
.panel {{ display:grid; grid-template-columns: repeat(auto-fit,minmax(18rem,1fr)); gap:.8rem; margin:.4rem 0 .8rem; }}
.beh {{ border:1px solid var(--line); border-radius:10px; padding:.7rem .8rem; }}
.beh-head {{ display:flex; justify-content:space-between; }}
.beh-rate {{ font-weight:800; }}
.bar {{ background: var(--soft); border-radius:4px; height:8px; margin:.4rem 0; overflow:hidden; }}
.bar span {{ display:block; height:100%; background: var(--good); }}
.rubric {{ font-size:12px; opacity:.7; margin:.2rem 0 .4rem; }}
.dots {{ display:flex; flex-wrap:wrap; gap:3px; }}
.dot {{ width:18px; height:18px; line-height:18px; text-align:center; border-radius:4px; font-size:11px; text-decoration:none; color:#fff; }}
.dot.y {{ background: var(--good); }} .dot.n {{ background: var(--bad); }}
.rollout {{ border:1px solid var(--line); border-radius:10px; margin:.4rem 0; padding:.1rem .6rem; }}
.rollout > summary {{ cursor:pointer; padding:.5rem .2rem; font-size:13px; }}
.rmeta {{ float:right; opacity:.6; font-size:12px; }}
.badge {{ display:inline-block; min-width:3.2rem; text-align:center; border-radius:5px; padding:1px 6px; font-weight:700; font-size:12px; color:#fff; }}
.badge.good {{ background: var(--good); }} .badge.warn {{ background: var(--warn); }} .badge.bad {{ background: var(--bad); }}
.verdicts {{ border-collapse:collapse; width:100%; margin:.4rem 0; font-size:13px; }}
.verdicts td, .verdicts th {{ border-bottom:1px solid var(--line); padding:3px 6px; vertical-align:top; text-align:left; }}
.verdicts tr.y td:first-child {{ color: var(--good); }} .verdicts tr.n td:first-child {{ color: var(--bad); }}
.tl-h {{ font-size:.8rem; opacity:.6; text-transform:uppercase; letter-spacing:.04em; margin:.8rem 0 .3rem; }}
.timeline {{ border-left:2px solid var(--line); padding-left:.7rem; margin-left:.2rem; }}
.step {{ margin:.45rem 0; }}
.step .lbl {{ font-size:11px; opacity:.55; text-transform:uppercase; letter-spacing:.04em; }}
.step.text .prose, .step.final .prose {{ white-space:pre-wrap; }}
.step.final {{ border:1px solid var(--good); border-radius:8px; padding:.5rem .7rem; background:rgba(46,157,78,.08); }}
.step details > summary {{ cursor:pointer; font-size:12px; }}
.step.tooluse > summary {{ color:#3b82f6; }}
.step.toolresult.error {{ }}
.step.toolresult.error > summary {{ color: var(--bad); }}
.step.thinking {{ opacity:.7; }}
pre {{ white-space:pre-wrap; word-break:break-word; background:var(--soft); padding:8px; border-radius:6px; max-height:32rem; overflow:auto; margin:.25rem 0; font-size:12px; }}
.fallback {{ max-height:none; }}
.toolbar {{ position:sticky; top:0; background:Canvas; padding:.4rem 0; border-bottom:1px solid var(--line); margin-bottom:.6rem; z-index:5; }}
.toolbar button {{ font:inherit; font-size:12px; padding:3px 9px; border:1px solid var(--line); border-radius:6px; background:var(--soft); cursor:pointer; }}
a {{ color:#3b82f6; }}
</style></head>
<body>
<h1>system-prompt eval scorecard</h1>
<div class="chips">{meta_chips}</div>
<div class="cards">{cards}</div>
<div class="toolbar">
  <button onclick="document.querySelectorAll('details.rollout,details.step').forEach(d=>d.open=true)">expand all</button>
  <button onclick="document.querySelectorAll('details.rollout,details.step').forEach(d=>d.open=false)">collapse all</button>
  <span class="muted"> — click any run to see its full action timeline; click a behavior dot to jump to that run</span>
</div>
{sections}
</body></html>"""
