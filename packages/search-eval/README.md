# search-eval

Measure how good [`search`](../search) is, the way the search-eval community
actually measures retrieval. `search-eval` runs a versioned query set against the
real `search` engine over a fixed corpus and scores it, in two tiers that mirror
how [Exa](https://exa.ai) runs its "open evals":

- **Tier A — retrieval grading.** Rank the corpus for each query and score the
  ranking against gold labels (nDCG@10, Recall@k, MRR), plus an optional
  label-free LLM relevance judge. This is the cheap, reproducible signal you can
  gate on.
- **Tier B — agentic downstream.** Give a headless `claude -p` agent exactly one
  tool, a corpus-scoped search, and a question it cannot answer from memory; then
  grade whether it answered correctly. This is the "does our search actually help
  an agent" signal, Exa's RAG/SimpleQA mode.

```sh
# Tier A: grade rankings against the gold query set (LLM judge on by default).
nix run .#search-eval -- retrieval

# Tier A without the judge (rank metrics only, no API key needed for grading).
nix run .#search-eval -- retrieval --no-judge

# Tier B: run claude -p with only search, grade the answers.
nix run .#search-eval -- agentic

# Both, plus write the JSON report.
nix run .#search-eval -- all --json-out report.json
```

## What you need

- A Mixedbread credential for `search`: `MXBAI_API_KEY`, or a `mgrep login`
  token. Without it `search` cannot index or query.
- `ANTHROPIC_API_KEY` for the LLM judge (both tiers) and the `claude -p` agent
  (Tier B). Pass `--no-judge` to skip grading in Tier A.

These are live, networked evals run on demand or nightly, not hermetic unit
tests. The one part that *is* hermetic, the ranking metrics, is unit-tested
offline and is what CI gates (`passthru.tests.metrics`).

## The corpus and the query sets

The corpus is a small committed fixture under [`corpus/`](corpus): a dozen tiny
modules on distinct topics (retry backoff, rate limiting, an LRU cache, a
connection pool, ...) with **specific, made-up constants**. That last part is
deliberate: per Exa's anti-contamination recipe, the answers cannot be guessed
from a model's parametric memory, so a correct answer is evidence the *retrieval*
worked, not that the model already knew it.

The query sets are data, one JSON object per line so a diff shows exactly what
changed:

- [`datasets/retrieval.jsonl`](datasets/retrieval.jsonl): `{id, query, relevant}`
  where `relevant` maps a corpus path to a graded relevance (`1` binary, or a
  small integer). Because we own the corpus we use real gold labels here, which
  Exa prefers over LLM grading wherever enumerating the relevant docs is
  feasible.
- [`datasets/tasks.jsonl`](datasets/tasks.jsonl): `{id, task, answer}` for the
  agentic tier.

Point either tier at your own set with `--dataset`, or at any checkout with
`--corpus`.

## Metrics

| Metric | What it tells you |
| --- | --- |
| **nDCG@10** | The headline ranking metric (graded, position-discounted), the BEIR/MTEB standard Exa and the community report. |
| **Recall@5 / @10** | Coverage: did the relevant docs land in the top k. |
| **MRR** | Reciprocal rank of the first relevant doc; the single-answer view. |
| **jNDCG** | Label-free cross-check: nDCG computed over an LLM judge's per-result relevance scores. A flat mean would punish a perfect ranking that has one relevant result and an off-topic tail, so we discount by rank like Exa. |
| **accuracy** (Tier B) | Fraction of tasks the agent answered correctly using only search. |

Use `--fail-under <x>` to make a tier exit non-zero when the headline metric
(nDCG@10 for `retrieval`, accuracy for `agentic`) falls below a threshold, for a
CI gate once numbers are baselined.

## The LLM judge

Grading follows the robustness measures from the LLM-as-judge literature: a
forced tool call for structured output, `temperature=0`, an explicit calibrated
rubric, and a `reasoning` field emitted before the score so verdicts are
chain-of-thought-grounded. Grading is pointwise (one result at a time), so
position bias does not apply. The default judge model is a mid-tier Claude, the
analog of the GPT-4.1 grader Exa reports; override with `--judge-model`.

## Isolation backends (Tier B)

The agent runs behind a pluggable backend:

- `--backend local` (default) runs each `claude -p` in a throwaway empty temp
  directory whose only declared tool is a one-tool MCP search server
  ([`mcp_server.py`](src/search_eval/mcp_server.py)), with Bash and every
  file/web reader denied and the corpus path kept out of the agent's view (the
  MCP config lives outside the working directory; the prompt never names a path).
  This is **best-effort isolation for a cooperative agent, not a security
  boundary.** Claude Code still executes read-only shell regardless of the tool
  allow/deny lists, so an adversarial agent that hunts the filesystem could read
  the corpus without searching. For a cooperative agent answering a question it
  has only the search tool and no path, so the number reflects search; treat it
  accordingly and use the airtight backend below when the isolation must hold.
- `--backend ixvm` is the typed seam for the **airtight** boundary: run each
  agent inside a disposable ix VM whose only view of the corpus is the search
  tool (the same shape Symphony uses to run Codex). It is not implemented yet and
  returns an explicit error rather than silently falling back: ix VMs run on
  x86_64-linux compute nodes, so wiring this up belongs in a follow-up. The
  interface lives in [`agent.py`](src/search_eval/agent.py).

## Why Python here

Core search logic stays in Rust (`search-core`); this is an evaluation harness,
not domain logic. It is iteration-heavy, grades by calling an LLM, and drives
`claude -p` as a subprocess, all of which Python expresses directly, and the
reference harness it borrows from ([`exa-labs/benchmarks`](https://github.com/exa-labs/benchmarks),
MIT) is Python. The machine-readable contract it consumes, `search --json`, lives
at the owner (the `search` CLI), so this harness adds no domain logic of its own.

## Background

The methodology here is drawn from Exa's writing on how they evaluate neural
search:

- [Evals at Exa](https://exa.ai/blog/evals-at-exa): the open-eval philosophy
  (a query list graded by an LLM), the grading rubric, and aggregation choices.
- [WebCode](https://exa.ai/blog/webcode): the code-search harness that separates
  retrieval quality from answer quality and grades groundedness
  discriminatively.
- [Evaluating Exa search](https://exa.ai/docs/reference/evaluating-exa-search):
  fairness controls (compare within a latency class, use each engine's real
  returned content, document every config).
