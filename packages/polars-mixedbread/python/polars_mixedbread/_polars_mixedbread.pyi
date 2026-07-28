"""Type stub for the PyO3 extension module backing `polars_mixedbread`."""

from typing import Any

__version__: str

def search_mixedbread(
    stores: list[str],
    query: str,
    top_k: int = ...,
    base_url: str | None = ...,
    rerank: bool = ...,
    agentic: bool = ...,
    score_threshold: float | None = ...,
    filters: str | None = ...,
    reranker: str | None = ...,
) -> dict[str, list[Any]]:
    """Run a Mixedbread store search, returning a dict of the six source columns.

    `filters` is the metadata filter as a JSON string. The wrapper
    `scan_mixedbread` builds it from the Polars predicate; see that function.
    """
