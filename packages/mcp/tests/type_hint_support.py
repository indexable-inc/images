"""Shared test helper mirroring the ruff ANN gate: assert every public
function fully annotates its parameters and return type. Copied next to the
per-module tests that use it (see each module's module.nix).
"""

from __future__ import annotations

import inspect
from collections.abc import Callable, Iterable
from typing import Any


def assert_type_hints_explicit(funcs: Iterable[Callable[..., Any]]) -> None:
    for func in funcs:
        sig = inspect.signature(func)
        assert sig.return_annotation is not inspect.Signature.empty, (
            f"{func.__name__} missing return annotation"
        )
        for pname, param in sig.parameters.items():
            assert param.annotation is not inspect.Parameter.empty, (
                f"{func.__name__}({pname}) missing annotation"
            )
