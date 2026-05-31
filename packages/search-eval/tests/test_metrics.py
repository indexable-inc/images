"""Offline, deterministic checks for the ranking metrics.

Runnable as a plain script (``python tests/test_metrics.py``) so the Nix build
can exercise it against the installed package with no test runner or network.
"""

from __future__ import annotations

import math

from search_eval.metrics import (
    mrr,
    ndcg_at_k,
    ndcg_from_scores,
    precision_at_k,
    recall_at_k,
    score_ranking,
)


def test_perfect_ranking() -> None:
    gold = {"a.py": 1.0}
    assert ndcg_at_k(["a.py", "b.py"], gold, 10) == 1.0
    assert mrr(["a.py", "b.py"], gold) == 1.0
    assert recall_at_k(["a.py"], gold, 10) == 1.0


def test_rank_discount() -> None:
    gold = {"a.py": 1.0}
    # Relevant doc at rank 2: gain 1 / log2(3).
    expected = (1.0 / math.log2(3)) / 1.0
    assert math.isclose(ndcg_at_k(["x.py", "a.py"], gold, 10), expected, rel_tol=1e-9)
    assert mrr(["x.py", "a.py"], gold) == 0.5


def test_missing_relevant() -> None:
    gold = {"a.py": 1.0}
    assert ndcg_at_k(["x.py", "y.py"], gold, 10) == 0.0
    assert recall_at_k(["x.py"], gold, 10) == 0.0
    assert mrr(["x.py"], gold) == 0.0


def test_empty_gold_is_zero() -> None:
    assert ndcg_at_k(["a.py"], {}, 10) == 0.0
    assert recall_at_k(["a.py"], {}, 10) == 0.0


def test_graded_relevance_orders_by_gain() -> None:
    # A high-grade doc above a low-grade doc beats the reverse order.
    gold = {"hi.py": 2.0, "lo.py": 1.0}
    good = ndcg_at_k(["hi.py", "lo.py"], gold, 10)
    worse = ndcg_at_k(["lo.py", "hi.py"], gold, 10)
    assert good == 1.0
    assert worse < good


def test_precision_and_recall_at_k() -> None:
    gold = {"a.py": 1.0, "b.py": 1.0}
    ranked = ["a.py", "x.py", "b.py", "y.py"]
    assert precision_at_k(ranked, gold, 2) == 0.5
    assert recall_at_k(ranked, gold, 2) == 0.5
    assert recall_at_k(ranked, gold, 3) == 1.0


def test_ndcg_from_scores() -> None:
    # A relevant result at rank 1 with an off-topic tail still scores 1.0.
    assert ndcg_from_scores([1.0, 0.0, 0.0]) == 1.0
    # Burying the relevant result discounts it below 1.0.
    assert ndcg_from_scores([0.0, 1.0, 0.0]) < 1.0
    # All-zero scores are 0.0, not a divide-by-zero.
    assert ndcg_from_scores([0.0, 0.0]) == 0.0


def test_score_ranking_bundle() -> None:
    scores = score_ranking(["a.py"], {"a.py": 1.0})
    d = scores.as_dict()
    assert set(d) == {"ndcg@10", "recall@5", "recall@10", "mrr", "precision@5"}
    assert d["ndcg@10"] == 1.0
    assert d["mrr"] == 1.0


def _main() -> None:
    tests = [v for name, v in sorted(globals().items()) if name.startswith("test_")]
    for test in tests:
        test()
    print(f"ok: {len(tests)} metric tests passed")


if __name__ == "__main__":
    _main()
