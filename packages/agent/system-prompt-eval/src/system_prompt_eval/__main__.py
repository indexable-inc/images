"""Entry point so `python -m system_prompt_eval` mirrors the console script."""

from .cli import main

if __name__ == "__main__":
    raise SystemExit(main())
