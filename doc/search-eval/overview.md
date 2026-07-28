# search-eval

`packages/search-eval` measures how good [`search`](../search/overview.md) is,
the way the neural-search community (Exa's "open evals") measures retrieval. It
runs a versioned query set against the real `search` engine over a fixed,
committed corpus and scores it. Python (an evaluation harness, not domain logic),
`nix run .#search-eval` (`default.nix`, `package.nix`).

## Two tiers

- **Tier A, retrieval grading** (`retrieval`): rank the corpus for each query and
  score the ranking against gold labels (nDCG@10, Recall@k, MRR), plus an
  optional label-free LLM relevance judge (README:8-11). The cheap, reproducible
  signal to gate on; the ranking metrics are the only hermetic, CI-gated part.
- **Tier B, agentic downstream** (`agentic`): give a headless `claude -p` agent
  exactly one tool, a corpus-scoped search, and a question it cannot answer from
  memory, then grade whether it answered correctly (README:12-15). The "does
  search actually help an agent" signal.

```sh
nix run .#search-eval -- retrieval            # Tier A (LLM judge on by default)
nix run .#search-eval -- retrieval --no-judge # rank metrics only, no API key
nix run .#search-eval -- agentic              # Tier B
nix run .#search-eval -- all --json-out report.json
```

## CLI (`src/search_eval/cli.py`)

`_build_parser` (`cli.py:53`) defines `retrieval`, `agentic`, and `all`. Common
flags (`cli.py:41`): `--search-bin` (default `search` on PATH), `--corpus`
(default the packaged fixture), `--store`, `--max-count`, `--limit`,
`--json-out`, `--judge-model`. `retrieval` adds `--no-rerank` / `--reranker`,
`--no-judge`, `--judge-top-n`, `--dataset`, `--fail-under` (exit 1 when mean
nDCG@10 falls below). `agentic` adds `--claude-bin`, `--backend local|ixvm`,
`--agent-model`, `--fail-under` (on accuracy). `main` (`cli.py:145`) wires the
thresholds into the exit code.

## Corpus, datasets, metrics

The corpus is a committed fixture under `corpus/`: a dozen tiny modules on
distinct topics with deliberately made-up constants, so a correct answer is
evidence the retrieval worked rather than the model's memory (Exa's
anti-contamination recipe, README:42-49). Query sets are JSONL data:
`datasets/retrieval.jsonl` (`{id, query, relevant}` with graded relevance) and
`datasets/tasks.jsonl` (`{id, task, answer}` for Tier B). Point either tier at
your own set with `--dataset` or any checkout with `--corpus`.

Metrics (README:65-73): **nDCG@10** (the headline graded, position-discounted
metric), **Recall@5/@10**, **MRR**, **jNDCG** (rank-discounted nDCG over the LLM
judge's per-result scores), and Tier B **accuracy**. The LLM judge follows the
LLM-as-judge robustness measures: a forced tool call for structured output,
`temperature=0`, a calibrated rubric, and a `reasoning` field before the score;
grading is pointwise so position bias does not apply (README:79-86).

## Agent isolation (Tier B)

The agent runs behind a pluggable backend (README:88-108):

- `--backend local` (default): each `claude -p` runs in a throwaway temp dir
  whose only declared tool is a one-tool MCP search server (`mcp_server.py`),
  with Bash and file/web readers denied and the corpus path kept out of view.
  This is best-effort isolation for a cooperative agent, not a security boundary
  (Claude Code can still run read-only shell), so the number reflects search.
- `--backend ixvm`: the typed seam for the airtight boundary (run each agent in a
  disposable ix VM). Not implemented yet; it returns an explicit error rather
  than silently falling back (`agent.py`).

The agent's search tool runs with `--no-sync`, so the local backend indexes the
corpus first via `SearchBackend.warmup()` before running tasks (`cli.py:133`).

## Build

`default.nix` builds the uv application with `ix.buildUvApplication` and wraps it
so the real `search` binary (from the workspace graph) and `claude-code` are on
its PATH, with the committed corpus/datasets staged at `SEARCH_EVAL_DATA_DIR`
(`default.nix:8-55`). The offline ranking metrics are unit-tested as the
CI-gating `passthru.tests.metrics`, plus a `printsHelp` smoke test
(`default.nix:59-87`). It depends on the live `search` engine, so Tier numbers
need a real Mixedbread credential and (for the judge / agent)
`ANTHROPIC_API_KEY`.
