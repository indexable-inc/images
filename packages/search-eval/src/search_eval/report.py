"""Aggregate results and render them as a table and a JSON report.

The JSON report is the durable artifact (pin it in CI, diff it across runs); the
table is the at-a-glance human view. Metric names are spelled with their `@k`
cutoff so a number is never ambiguous.
"""

from __future__ import annotations

from collections.abc import Sequence

from .model import RetrievalResult, TaskResult

_METRIC_ORDER = ["ndcg@10", "recall@5", "recall@10", "mrr", "precision@5"]


def summarize_retrieval(results: Sequence[RetrievalResult]) -> dict[str, float]:
    """Mean of each metric (and judge relevance, if graded) over all cases."""
    if not results:
        return {}
    summary = {
        name: sum(r.metrics.get(name, 0.0) for r in results) / len(results)
        for name in _METRIC_ORDER
    }
    graded = [r.judge_relevance for r in results if r.judge_relevance is not None]
    if graded:
        summary["judge_relevance"] = sum(graded) / len(graded)
    return summary


def summarize_agentic(results: Sequence[TaskResult]) -> dict[str, float]:
    """Agentic accuracy: fraction of tasks the agent answered correctly."""
    if not results:
        return {}
    correct = sum(1 for r in results if r.correct)
    errored = sum(1 for r in results if r.error)
    return {
        "accuracy": correct / len(results),
        "correct": float(correct),
        "errored": float(errored),
        "total": float(len(results)),
    }


def retrieval_report(results: Sequence[RetrievalResult]) -> dict[str, object]:
    return {
        "tier": "retrieval",
        "summary": summarize_retrieval(results),
        "cases": [
            {
                "id": r.case.id,
                "query": r.case.query,
                "retrieved": r.retrieved[:10],
                "metrics": r.metrics,
                "judge_relevance": r.judge_relevance,
            }
            for r in results
        ],
    }


def agentic_report(results: Sequence[TaskResult]) -> dict[str, object]:
    return {
        "tier": "agentic",
        "summary": summarize_agentic(results),
        "cases": [
            {
                "id": r.case.id,
                "task": r.case.task,
                "gold": r.case.answer,
                "answer": r.answer,
                "correct": r.correct,
                "reasoning": r.reasoning,
                "error": r.error,
            }
            for r in results
        ],
    }


def render_retrieval_table(results: Sequence[RetrievalResult]) -> str:
    """A fixed-width table: one row per case, then the mean row."""
    header = f"{'case':<22} {'nDCG@10':>8} {'R@5':>6} {'R@10':>6} {'MRR':>6} {'jNDCG':>6}"
    lines = [header, "-" * len(header)]
    for r in results:
        jr = "-" if r.judge_relevance is None else f"{r.judge_relevance:.2f}"
        lines.append(
            f"{r.case.id:<22} {r.metrics['ndcg@10']:>8.3f} {r.metrics['recall@5']:>6.2f} "
            f"{r.metrics['recall@10']:>6.2f} {r.metrics['mrr']:>6.2f} {jr:>6}"
        )
    summary = summarize_retrieval(results)
    if summary:
        jr = f"{summary['judge_relevance']:.2f}" if "judge_relevance" in summary else "-"
        lines.append("-" * len(header))
        lines.append(
            f"{'MEAN':<22} {summary['ndcg@10']:>8.3f} {summary['recall@5']:>6.2f} "
            f"{summary['recall@10']:>6.2f} {summary['mrr']:>6.2f} {jr:>6}"
        )
    return "\n".join(lines)


def render_agentic_table(results: Sequence[TaskResult]) -> str:
    header = f"{'task':<22} {'correct':>8}  answer"
    lines = [header, "-" * len(header)]
    for r in results:
        mark = "ERROR" if r.error else ("yes" if r.correct else "no")
        detail = r.error or r.answer
        lines.append(f"{r.case.id:<22} {mark:>8}  {detail[:48]}")
    summary = summarize_agentic(results)
    if summary:
        lines.append("-" * len(header))
        lines.append(
            f"{'ACCURACY':<22} {summary['accuracy']:>8.2f}  "
            f"({int(summary['correct'])}/{int(summary['total'])}, "
            f"{int(summary['errored'])} errored)"
        )
    return "\n".join(lines)
