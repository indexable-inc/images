"""Run the `search` CLI and parse its machine-readable output.

The harness evaluates the real `search` binary through its ``--json`` mode rather
than reimplementing retrieval. Results come back as a JSON array of hits, parsed
into [`Hit`][search_eval.model.Hit] in rank order.

Authentication is the binary's concern: it reads ``MXBAI_API_KEY`` or the token
written by ``mgrep login``. A failed search raises [`SearchError`] with the
binary's stderr, never a silent empty result.
"""

from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass
from pathlib import Path

from .model import Hit


class SearchError(RuntimeError):
    """The `search` binary failed or returned output that did not parse."""


@dataclass(frozen=True, slots=True)
class SearchBackend:
    """Invoke `search --json` against a fixed corpus directory."""

    corpus: Path
    search_bin: str = "search"
    sources: tuple[str, ...] = ("code",)
    store: str | None = None
    max_count: int = 10
    rerank: bool = True
    reranker: str | None = None
    timeout_seconds: float = 180.0

    def _base_args(self, no_sync: bool) -> list[str]:
        args = [self.search_bin, "--json", "-m", str(self.max_count)]
        for source in self.sources:
            args += ["--source", source]
        if not self.rerank:
            args.append("--no-rerank")
        elif self.reranker:
            args += ["--reranker", self.reranker]
        if no_sync:
            args.append("--no-sync")
        if self.store:
            args += ["--store", self.store]
        return args

    def warmup(self) -> None:
        """Index the corpus once so later ``--no-sync`` searches are fast.

        Embedding is content-addressed, so this only pays the upload/embed cost
        for content the store has not seen before.
        """
        self._run(self._base_args(no_sync=False) + ["warm up the index", str(self.corpus)])

    def search(self, query: str, *, no_sync: bool = False) -> list[Hit]:
        """Return the ranked hits for ``query`` over the corpus."""
        out = self._run(self._base_args(no_sync) + [query, str(self.corpus)])
        try:
            raw = json.loads(out or "[]")
        except json.JSONDecodeError as exc:
            raise SearchError(f"search did not return JSON: {exc}\noutput: {out[:400]!r}") from exc
        if not isinstance(raw, list):
            raise SearchError(f"expected a JSON array, got {type(raw).__name__}")
        return [Hit.from_json(obj) for obj in raw]

    def _run(self, args: list[str]) -> str:
        try:
            proc = subprocess.run(
                args,
                capture_output=True,
                text=True,
                timeout=self.timeout_seconds,
                check=False,
            )
        except FileNotFoundError as exc:
            raise SearchError(
                f"`{self.search_bin}` not found on PATH; build it with `nix build .#search` "
                "or pass --search-bin"
            ) from exc
        except subprocess.TimeoutExpired as exc:
            raise SearchError(f"search timed out after {self.timeout_seconds}s") from exc
        if proc.returncode != 0:
            raise SearchError(
                f"search exited {proc.returncode}: {proc.stderr.strip() or '(no stderr)'}"
            )
        return proc.stdout.strip()
