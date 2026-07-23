"""Orchestrate a tier: search/agent, then grade, then collect results.

Kept free of presentation and argument parsing so it can be driven from the CLI
or a notebook. The two entry points mirror Exa's two evaluation modes.
"""

from __future__ import annotations

from collections.abc import Callable, Sequence

from .agent import LocalBackend, IxVmBackend
from .backend import SearchBackend
from .judge import Judge
from .metrics import ndcg_from_scores, score_ranking
from .model import RetrievalCase, RetrievalResult, TaskCase, TaskResult

Progress = Callable[[str], None]


def _noop(_: str) -> None:
    return None


def run_retrieval(
    cases: Sequence[RetrievalCase],
    backend: SearchBackend,
    judge: Judge | None,
    *,
    judge_top_n: int = 3,
    progress: Progress = _noop,
) -> list[RetrievalResult]:
    """Tier A: rank with `search`, score against gold, optionally LLM-grade hits."""
    progress("indexing corpus")
    backend.warmup()
    results: list[RetrievalResult] = []
    for case in cases:
        progress(f"retrieval: {case.id}")
        hits = backend.search(case.query, no_sync=True)
        # De-duplicate by path, keeping the best (first) rank: a real checkout
        # can return several chunks of one file, which would otherwise count a
        # relevant doc more than once and miscalibrate nDCG/precision.
        retrieved = list(dict.fromkeys(hit.path for hit in hits))
        result = RetrievalResult(
            case=case,
            retrieved=retrieved,
            metrics=score_ranking(retrieved, case.relevant).as_dict(),
        )
        if judge is not None and hits:
            grades = [judge.grade_relevance(case.query, hit) for hit in hits[:judge_top_n]]
            result.grades = grades
            # Rank-aware, label-free: nDCG over the judge's per-result scores.
            result.judge_relevance = ndcg_from_scores([g.score for g in grades])
        results.append(result)
    return results


def run_agentic(
    cases: Sequence[TaskCase],
    agent: LocalBackend | IxVmBackend,
    judge: Judge,
    *,
    progress: Progress = _noop,
) -> list[TaskResult]:
    """Tier B: answer each task via `claude -p`, then grade correctness."""
    results: list[TaskResult] = []
    for case in cases:
        progress(f"agentic: {case.id}")
        try:
            answer = agent.run_task(case)
        except Exception as exc:
            results.append(
                TaskResult(case=case, answer="", correct=False, reasoning="", error=str(exc))
            )
            continue
        verdict = judge.grade_correctness(case.task, case.answer, answer)
        results.append(
            TaskResult(
                case=case,
                answer=answer,
                correct=verdict.passed,
                reasoning=verdict.reasoning,
            )
        )
    return results
