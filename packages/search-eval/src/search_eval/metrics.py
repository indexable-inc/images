"""Ranking metrics for retrieval evaluation.

Pure functions over a ranked list of retrieved document ids and a `gold` map of
`doc_id -> graded relevance`. No I/O, no network, no clock: this module is the
deterministic, unit-tested core of the harness and the only part that gates CI.

Conventions follow BEIR/TREC (the reference points Exa and the wider retrieval
community report against):

- **nDCG@k** uses exponential gain ``2**rel - 1`` with a ``log2(rank + 1)``
  position discount, normalized by the ideal ranking. Binary relevance
  (``rel = 1``) reduces to the familiar ``1 / log2(rank + 1)`` gain.
- **Recall@k / Precision@k / MRR** treat any ``rel > 0`` doc as relevant.

A `gold` value is a relevance grade: ``1`` for binary "relevant", or a small
integer (e.g. ``0..3``) for graded relevance. Graded labels give nDCG its
discriminative power (a perfect hit outranks a loosely related one); see the
ZeroEntropy MTEB re-annotation argument referenced in the README.
"""

from __future__ import annotations

import math
from collections.abc import Mapping, Sequence

__all__ = [
    "RetrievalScores",
    "dcg",
    "mrr",
    "ndcg_at_k",
    "ndcg_from_scores",
    "precision_at_k",
    "recall_at_k",
    "score_ranking",
]

from dataclasses import dataclass


def _gain(rel: float) -> float:
    """Exponential gain ``2**rel - 1``; ``rel <= 0`` contributes nothing.

    ``math.pow`` (rather than the ``**`` operator) keeps the result statically
    typed as ``float``: ``float.__pow__`` is typed to return ``Any`` for a
    non-literal exponent, which would defeat strict return-type checking.
    """
    if rel <= 0:
        return 0.0
    return math.pow(2.0, rel) - 1.0


def dcg(relevances: Sequence[float]) -> float:
    """Discounted cumulative gain of a ranked relevance sequence.

    Rank is 1-based, so position ``i`` (0-based) is discounted by
    ``log2(i + 2)``.
    """
    return sum(_gain(rel) / math.log2(i + 2) for i, rel in enumerate(relevances))


def ndcg_at_k(retrieved: Sequence[str], gold: Mapping[str, float], k: int) -> float:
    """Normalized DCG over the top ``k`` retrieved ids.

    Returns ``0.0`` when no relevant documents exist (an empty `gold`), matching
    the convention that an undefined ideal DCG scores zero rather than raising.
    """
    if k <= 0:
        return 0.0
    ranked = [gold.get(doc_id, 0.0) for doc_id in retrieved[:k]]
    actual = dcg(ranked)
    ideal_rels = sorted((rel for rel in gold.values() if rel > 0), reverse=True)[:k]
    ideal = dcg(ideal_rels)
    if ideal == 0.0:
        return 0.0
    return actual / ideal


def ndcg_from_scores(scores: Sequence[float], k: int = 10) -> float:
    """Label-free nDCG over per-result relevance scores (e.g. an LLM judge's).

    Treats ``scores`` as the graded relevance of the ranking in order, and
    normalizes by the same scores sorted descending. A ranking that puts its
    most relevant result first scores near ``1.0``; burying it discounts the
    result. Returns ``0.0`` when every score is zero.

    This is the aggregation Exa uses for label-free grading: a flat mean would
    punish a perfect ranking that has one relevant result and an off-topic tail.
    """
    if k <= 0:
        return 0.0
    actual = dcg(list(scores)[:k])
    ideal = dcg(sorted(scores, reverse=True)[:k])
    if ideal == 0.0:
        return 0.0
    return actual / ideal


def recall_at_k(retrieved: Sequence[str], gold: Mapping[str, float], k: int) -> float:
    """Fraction of all relevant docs that appear in the top ``k``."""
    relevant = {doc_id for doc_id, rel in gold.items() if rel > 0}
    if not relevant:
        return 0.0
    found = relevant.intersection(retrieved[:k])
    return len(found) / len(relevant)


def precision_at_k(retrieved: Sequence[str], gold: Mapping[str, float], k: int) -> float:
    """Fraction of the top ``k`` retrieved docs that are relevant."""
    if k <= 0:
        return 0.0
    relevant = {doc_id for doc_id, rel in gold.items() if rel > 0}
    found = sum(1 for doc_id in retrieved[:k] if doc_id in relevant)
    return found / k


def mrr(retrieved: Sequence[str], gold: Mapping[str, float]) -> float:
    """Reciprocal rank of the first relevant doc; ``0.0`` if none is retrieved."""
    relevant = {doc_id for doc_id, rel in gold.items() if rel > 0}
    for rank, doc_id in enumerate(retrieved, start=1):
        if doc_id in relevant:
            return 1.0 / rank
    return 0.0


@dataclass(frozen=True, slots=True)
class RetrievalScores:
    """The standard metric bundle for one query's ranking."""

    ndcg_at_10: float
    recall_at_5: float
    recall_at_10: float
    mrr: float
    precision_at_5: float

    def as_dict(self) -> dict[str, float]:
        return {
            "ndcg@10": self.ndcg_at_10,
            "recall@5": self.recall_at_5,
            "recall@10": self.recall_at_10,
            "mrr": self.mrr,
            "precision@5": self.precision_at_5,
        }


def score_ranking(retrieved: Sequence[str], gold: Mapping[str, float]) -> RetrievalScores:
    """Compute the full metric bundle for one ranked result list."""
    return RetrievalScores(
        ndcg_at_10=ndcg_at_k(retrieved, gold, 10),
        recall_at_5=recall_at_k(retrieved, gold, 5),
        recall_at_10=recall_at_k(retrieved, gold, 10),
        mrr=mrr(retrieved, gold),
        precision_at_5=precision_at_k(retrieved, gold, 5),
    )
