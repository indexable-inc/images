"""Entry point so `python -m search_eval` mirrors the `search-eval` console script."""

from .cli import main

if __name__ == "__main__":
    raise SystemExit(main())
