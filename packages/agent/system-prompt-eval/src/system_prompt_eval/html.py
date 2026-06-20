"""Render an eval run as a single self-contained HTML scorecard.

Inline CSS, no external assets, dark-mode aware. Shows each eval's headline,
per-behavior pass rates, the longest all-pass streak, cost (time/tokens/$), and a
per-rollout table where each row expands to the judge's evidence and the raw
transcript. The same JSON that feeds this is the machine-readable raw data.
"""

from __future__ import annotations

import html
from collections.abc import Sequence

from .core import EvalReport


def _esc(value: object) -> str:
    return html.escape(str(value))


def _pct(value: float) -> str:
    return f"{value:.0%}"


def _chip(label: str, value: str) -> str:
    return f'<span class="chip"><b>{_esc(label)}</b> {_esc(value)}</span>'


def _cost_chips(summary: dict[str, object]) -> str:
    cost = summary.get("cost")
    if not isinstance(cost, dict):
        return ""
    out = []
    if isinstance(cost.get("mean_duration_s"), (int, float)):
        out.append(_chip("mean", f"{float(cost['mean_duration_s']):.0f}s"))
    if isinstance(cost.get("total_output_tokens"), (int, float)):
        out.append(_chip("out tok", f"{int(float(cost['total_output_tokens']))}"))
    if isinstance(cost.get("total_input_tokens"), (int, float)):
        out.append(_chip("in tok", f"{int(float(cost['total_input_tokens']))}"))
    if isinstance(cost.get("total_cost_usd"), (int, float)):
        out.append(_chip("cost", f"${float(cost['total_cost_usd']):.2f}"))
    return "".join(out)


def _behavior_table(summary: dict[str, object]) -> str:
    per = summary.get("per_behavior")
    if not isinstance(per, dict) or not per:
        return ""
    rows = "".join(
        f"<tr><td>{_esc(bid)}</td><td class='num'>{_pct(float(rate))}</td>"
        f"<td class='bar'><span style='width:{float(rate) * 100:.0f}%'></span></td></tr>"
        for bid, rate in per.items()
        if isinstance(rate, (int, float))
    )
    return f"<table class='beh'><tr><th>behavior</th><th>pass</th><th></th></tr>{rows}</table>"


def _case_row(case: dict[str, object]) -> str:
    cid = _esc(case.get("case_id", "?"))
    roll = _esc(case.get("rollout", ""))
    err = case.get("error")
    present = case.get("present")
    if err:
        status = "<span class='bad'>ERROR</span>"
        detail = _esc(err)
    elif isinstance(present, dict):
        ok = sum(1 for v in present.values() if v is True)
        status = f"<span class='{'good' if ok == len(present) else 'warn'}'>{ok}/{len(present)}</span>"
        detail = ", ".join(
            f"{_esc(b)}={'Y' if v else 'N'}" for b, v in present.items()
        )
    else:
        verdict = str(case.get("verdict", "?"))
        cls = {"validated": "good", "stale": "bad"}.get(verdict, "warn")
        status = f"<span class='{cls}'>{_esc(verdict)}</span>"
        detail = _esc(case.get("answer", ""))
    transcript = _esc(case.get("transcript", ""))
    evidence = case.get("evidence")
    ev_html = ""
    if isinstance(evidence, dict):
        ev_html = "<ul>" + "".join(
            f"<li><b>{_esc(b)}:</b> {_esc(t)}</li>" for b, t in evidence.items()
        ) + "</ul>"
    elif isinstance(evidence, str):
        ev_html = f"<p>{_esc(evidence)}</p>"
    dur = case.get("duration_ms")
    dur_s = f"{int(dur) / 1000:.0f}s" if isinstance(dur, (int, float)) else "-"
    return (
        f"<tr><td>{cid}</td><td class='num'>{roll}</td><td>{status}</td>"
        f"<td class='num'>{dur_s}</td><td>{detail}</td></tr>"
        f"<tr class='exp'><td colspan='5'><details><summary>evidence + transcript</summary>"
        f"{ev_html}<pre>{transcript}</pre></details></td></tr>"
    )


def _eval_section(rep: EvalReport) -> str:
    streak = "" if rep.longest_streak is None else _chip("streak", str(rep.longest_streak))
    head = (
        f"<h2>{_esc(rep.name)} <span class='headline'>{_pct(rep.headline)}</span></h2>"
        f"<div class='chips'>{streak}{_cost_chips(rep.summary)}</div>"
    )
    rows = "".join(_case_row(c) for c in rep.cases)
    table = (
        "<table class='cases'><tr><th>case</th><th>#</th><th>status</th>"
        f"<th>time</th><th>detail</th></tr>{rows}</table>"
    )
    return f"<section>{head}{_behavior_table(rep.summary)}{table}</section>"


def render_html(metadata: dict[str, object], reports: Sequence[EvalReport]) -> str:
    meta_chips = "".join(_chip(k, str(v)) for k, v in metadata.items())
    sections = "".join(_eval_section(r) for r in reports)
    return f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>system-prompt eval scorecard</title>
<style>
:root {{ color-scheme: light dark; }}
body {{ font: 14px/1.5 ui-monospace, "Berkeley Mono", Menlo, monospace; margin: 2rem auto; max-width: 60rem; padding: 0 1rem; }}
h1 {{ font-size: 1.4rem; }}
h2 {{ font-size: 1.1rem; margin-top: 2rem; border-top: 1px solid #8884; padding-top: 1rem; }}
.headline {{ float: right; font-size: 1.6rem; font-weight: 700; }}
.chips {{ margin: .5rem 0; }}
.chip {{ display: inline-block; background: #8881; border-radius: 6px; padding: 2px 8px; margin: 2px; font-size: 12px; }}
table {{ border-collapse: collapse; width: 100%; margin: .5rem 0; }}
th, td {{ text-align: left; padding: 4px 8px; border-bottom: 1px solid #8883; vertical-align: top; }}
.num {{ text-align: right; font-variant-numeric: tabular-nums; }}
.good {{ color: #2e9d4e; font-weight: 700; }}
.warn {{ color: #c98a00; font-weight: 700; }}
.bad {{ color: #d4493f; font-weight: 700; }}
.beh .bar {{ width: 40%; }}
.beh .bar span {{ display: inline-block; height: 10px; background: #2e9d4e; border-radius: 3px; }}
.exp td {{ border-bottom: 1px solid #8883; }}
details summary {{ cursor: pointer; color: #6a8; }}
pre {{ white-space: pre-wrap; word-break: break-word; background: #8881; padding: 8px; border-radius: 6px; max-height: 28rem; overflow: auto; }}
</style></head>
<body>
<h1>system-prompt eval scorecard</h1>
<div class="chips">{meta_chips}</div>
{sections}
</body></html>"""
