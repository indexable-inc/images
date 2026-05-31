"""`search-eval`: an Exa-style evaluation harness for the `search` engine.

Two tiers:

- ``retrieval`` grades `search` rankings against a gold query set (nDCG@10,
  Recall@k, MRR) plus an optional LLM relevance judge.
- ``agentic`` runs a headless ``claude -p`` whose only tool is search, then
  grades whether it answered correctly.

Both run against a committed fixture corpus, so results are reproducible and
free of training-data contamination.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from . import data
from .agent import IxVmBackend, LocalBackend
from .backend import SearchBackend
from .judge import DEFAULT_JUDGE_MODEL, Judge
from .paths import corpus_dir
from .report import (
    agentic_report,
    render_agentic_table,
    render_retrieval_table,
    retrieval_report,
    summarize_agentic,
    summarize_retrieval,
)
from .runner import run_agentic, run_retrieval


def _progress(message: str) -> None:
    print(f"  … {message}", file=sys.stderr, flush=True)


def _add_common(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--search-bin", default="search", help="`search` binary (default: PATH)")
    parser.add_argument(
        "--corpus", type=Path, default=None, help="corpus dir (default: packaged fixture)"
    )
    parser.add_argument("--store", default=None, help="Mixedbread store name (default: search's)")
    parser.add_argument("--max-count", type=int, default=10, help="results per query")
    parser.add_argument("--limit", type=int, default=None, help="only run the first N cases")
    parser.add_argument("--json-out", type=Path, default=None, help="write the JSON report here")
    parser.add_argument("--judge-model", default=DEFAULT_JUDGE_MODEL, help="LLM judge model")


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="search-eval", description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    retr = sub.add_parser("retrieval", help="Tier A: grade search rankings vs gold")
    _add_common(retr)
    retr.add_argument("--no-rerank", action="store_true", help="disable the second-stage reranker")
    retr.add_argument("--no-judge", action="store_true", help="skip the LLM relevance judge")
    retr.add_argument("--judge-top-n", type=int, default=3, help="hits to LLM-grade per query")
    retr.add_argument("--dataset", type=Path, default=None, help="override retrieval.jsonl")
    retr.add_argument("--fail-under", type=float, default=None, help="exit 1 if mean nDCG@10 below")

    agen = sub.add_parser("agentic", help="Tier B: claude -p with only search, graded")
    _add_common(agen)
    agen.add_argument("--claude-bin", default="claude", help="`claude` binary (default: PATH)")
    agen.add_argument("--backend", choices=("local", "ixvm"), default="local")
    agen.add_argument("--agent-model", default=None, help="model for the claude -p agent")
    agen.add_argument("--dataset", type=Path, default=None, help="override tasks.jsonl")
    agen.add_argument("--fail-under", type=float, default=None, help="exit 1 if accuracy below")

    allp = sub.add_parser("all", help="run both tiers")
    _add_common(allp)
    allp.add_argument("--claude-bin", default="claude")
    allp.add_argument("--backend", choices=("local", "ixvm"), default="local")

    return parser


def _search_backend(args: argparse.Namespace) -> SearchBackend:
    return SearchBackend(
        corpus=args.corpus or corpus_dir(),
        search_bin=args.search_bin,
        store=args.store,
        max_count=args.max_count,
        rerank=not getattr(args, "no_rerank", False),
    )


def _agent_backend(args: argparse.Namespace) -> LocalBackend | IxVmBackend:
    if args.backend == "ixvm":
        return IxVmBackend()
    return LocalBackend(
        corpus=args.corpus or corpus_dir(),
        search_bin=args.search_bin,
        claude_bin=args.claude_bin,
        max_results=args.max_count,
        agent_model=getattr(args, "agent_model", None),
    )


def _emit(report: dict[str, object], table: str, json_out: Path | None) -> None:
    print(table)
    if json_out is not None:
        json_out.write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(f"\nwrote {json_out}", file=sys.stderr)


def _run_retrieval(args: argparse.Namespace) -> dict[str, float]:
    cases = data.load_retrieval(args.dataset)
    if args.limit is not None:
        cases = cases[: args.limit]
    judge = None if getattr(args, "no_judge", False) else Judge(model=args.judge_model)
    results = run_retrieval(
        cases, _search_backend(args), judge, judge_top_n=args.judge_top_n, progress=_progress
    )
    _emit(retrieval_report(results), render_retrieval_table(results), args.json_out)
    return summarize_retrieval(results)


def _run_agentic(args: argparse.Namespace) -> dict[str, float]:
    cases = data.load_tasks(getattr(args, "dataset", None))
    if args.limit is not None:
        cases = cases[: args.limit]
    backend = _agent_backend(args)
    # The agent's search tool runs with --no-sync, so the corpus must be indexed
    # first. (The ixvm backend indexes inside the VM, so skip it here.)
    if isinstance(backend, LocalBackend):
        _progress("indexing corpus")
        _search_backend(args).warmup()
    results = run_agentic(
        cases, backend, Judge(model=args.judge_model), progress=_progress
    )
    _emit(agentic_report(results), render_agentic_table(results), args.json_out)
    return summarize_agentic(results)


def main(argv: list[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)

    if args.command == "retrieval":
        summary = _run_retrieval(args)
        if args.fail_under is not None and summary.get("ndcg@10", 0.0) < args.fail_under:
            print(f"FAIL: nDCG@10 {summary.get('ndcg@10', 0.0):.3f} < {args.fail_under}", file=sys.stderr)
            return 1
        return 0

    if args.command == "agentic":
        summary = _run_agentic(args)
        if args.fail_under is not None and summary.get("accuracy", 0.0) < args.fail_under:
            print(f"FAIL: accuracy {summary.get('accuracy', 0.0):.3f} < {args.fail_under}", file=sys.stderr)
            return 1
        return 0

    # `all`: run both tiers, no thresholds.
    args.no_rerank = False
    args.no_judge = False
    args.judge_top_n = 3
    args.dataset = None
    print("== Tier A: retrieval ==")
    _run_retrieval(args)
    print("\n== Tier B: agentic ==")
    args.json_out = None  # the per-tier reports already emitted; avoid clobber
    _run_agentic(args)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
