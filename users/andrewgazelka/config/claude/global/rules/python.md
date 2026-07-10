---
paths: "**/*.py, **/pyproject.toml, **/uv.lock, **/*.ipynb"
---

# Python

Use `uv` for everything. Never use `pip`, `pip install`, `poetry`, or `conda`.

## Package Management

```bash
uv add <pkg>              # Add dependency
uv add --dev <pkg>        # Add dev dependency
uv remove <pkg>           # Remove dependency
uv sync                   # Install all dependencies
uv lock                   # Update lockfile
```

## Running Code

```bash
uv run script.py          # Run script with project deps
uv run pytest             # Run pytest
uv run python -m module   # Run module
```

## Project Initialization

```bash
uv init                   # New project
uv init --lib             # New library
```

## Workspaces

For monorepos, use uv workspaces:

```toml
# pyproject.toml (root)
[tool.uv.workspace]
members = ["packages/*"]
```

Add workspace dependencies:

```toml
[project]
dependencies = ["my-core"]

[tool.uv.sources]
my-core = { workspace = true }
```

## Type Checking: `ty`

**Use `ty` for type checking, NOT mypy or pyright.**

```bash
ty check                  # Type check project
ty check src/             # Type check directory
```

Enable strict type checking in `pyproject.toml`:

```toml
[tool.ty.rules]
# Core strictness
unresolved-reference = "error"
invalid-type-form = "error"
invalid-type-expression = "error"
invalid-parameter-default = "error"
invalid-return-type = "error"
invalid-assignment = "error"
invalid-argument-type = "error"
missing-argument = "error"
unknown-argument = "error"
too-many-positional-arguments = "error"
parameter-already-assigned = "error"
incompatible-override = "error"
invalid-exception-caught = "error"
invalid-raise = "error"
possibly-unbound-attribute = "error"
possibly-unbound-import = "error"
```

## Linting & Formatting: `ruff`

```bash
ruff check --fix          # Lint and auto-fix
ruff format               # Format code
ruff check --fix && ruff format  # Full cleanup
```

## pyproject.toml

```toml
[project]
name = "my-project"
version = "0.1.0"
requires-python = ">=3.12"
dependencies = []

[dependency-groups]
dev = ["ruff", "pytest"]

[tool.ruff]
line-length = 100
target-version = "py312"

[tool.ruff.lint]
select = [
    "E",      # pycodestyle errors
    "F",      # pyflakes
    "I",      # isort
    "UP",     # pyupgrade
    "B",      # flake8-bugbear
    "SIM",    # flake8-simplify
    "RUF",    # ruff-specific
    "ANN",    # flake8-annotations
    "ASYNC",  # flake8-async
    "S",      # flake8-bandit (security)
    "PTH",    # flake8-use-pathlib
    "PERF",   # perflint
]
ignore = ["ANN101", "ANN102"]  # self/cls annotations

[tool.ruff.lint.isort]
force-single-line = true
```

## Jupyter Notebooks

Run notebooks with uv:

```bash
uv add --dev jupyterlab
uv run jupyter lab              # Interactive
uv run jupyter execute nb.ipynb # Execute notebook headlessly
```

Lint notebooks with ruff (add to pyproject.toml):

```toml
[tool.ruff]
extend-include = ["*.ipynb"]
```

```bash
ruff check --fix notebook.ipynb
ruff format notebook.ipynb
```

Type check notebooks with ty:

```bash
ty check notebook.ipynb
```

## Import Style

Prefer explicit imports:

```python
# GOOD
from pathlib import Path
from collections.abc import Sequence

# BAD
import pathlib
p = pathlib.Path("foo")
```

## Type Hints

Always use type hints for function signatures:

```python
def process(items: list[str], limit: int = 10) -> dict[str, int]:
    ...
```

Use modern syntax (Python 3.12+):

```python
# GOOD - modern
list[str]
dict[str, int]
str | None

# BAD - legacy
from typing import List, Dict, Optional
List[str]
Dict[str, int]
Optional[str]
```
