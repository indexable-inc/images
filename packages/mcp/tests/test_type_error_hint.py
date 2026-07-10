"""Tests for runtime._type_error_hint (ENG-2490).

_type_error_hint appends a live-signature hint to call-binding TypeErrors and
returns '' for non-binding TypeErrors or non-TypeError exceptions. (Originally
lived in test_fff_defaults_and_type_hints.py alongside fff-specific signature
tests; fff was removed, this runtime coverage was kept.)
"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))


def _get_hint_fn() -> object:
    """Return runtime._type_error_hint. Use the module-import style the test
    methods use (`import ... as rt`) so the file has one consistent import."""
    import ix_notebook_mcp.runtime as rt
    return rt._type_error_hint


# Module-level keyword-only function used as a test target for TypeError hints.
# Must be at module level so Python names it "kw_target" (not "Class.kw_target")
# in TypeError messages, matching the regex used by _type_error_hint.
def kw_target(*, query: str, mode: str = "plain", limit: int = 50) -> None:
    pass


def _kw_binding_error(*args: object, **kwargs: object) -> TypeError:
    try:
        kw_target(*args, **kwargs)
    except TypeError as e:
        return e
    raise AssertionError("expected TypeError")


class TestTypeErrorHint:
    """_type_error_hint appends a live signature hint to call-binding errors."""

    def setup_method(self) -> None:
        self.hint = _get_hint_fn()

    def test_unexpected_keyword_arg(self) -> None:
        exc = _kw_binding_error(max_results=20)
        import ix_notebook_mcp.runtime as rt
        old_ns = rt._user_ns
        try:
            rt._user_ns = {"kw_target": kw_target}
            h = self.hint(exc)
            assert "signature" in h
            assert "kw_target" in h
        finally:
            rt._user_ns = old_ns

    def test_missing_required_kwarg(self) -> None:
        # kw_target requires 'query' with no default.
        exc = _kw_binding_error()
        import ix_notebook_mcp.runtime as rt
        old_ns = rt._user_ns
        try:
            rt._user_ns = {"kw_target": kw_target}
            h = self.hint(exc)
            assert "signature" in h
        finally:
            rt._user_ns = old_ns

    def test_non_binding_typeerror_no_hint(self) -> None:
        """A TypeError raised inside a function body should get no hint."""
        exc = TypeError("cannot add str and int")
        h = self.hint(exc)
        assert h == ""

    def test_unknown_callable_no_hint(self) -> None:
        """A binding error for a name not in the namespace returns ''."""
        exc = TypeError("frobnicate() got an unexpected keyword argument 'x'")
        import ix_notebook_mcp.runtime as rt
        old_ns = rt._user_ns
        try:
            rt._user_ns = {}  # frobnicate not present
            h = self.hint(exc)
            assert h == ""
        finally:
            rt._user_ns = old_ns

    def test_module_qualified_lookup(self) -> None:
        """When the function lives on a module in the namespace (e.g. view.grep),
        the hint should find it via the module."""
        import types
        fake_mod = types.SimpleNamespace()
        fake_mod.grep = kw_target  # simulate a module-qualified callable

        exc = TypeError("grep() got an unexpected keyword argument 'max_results'")
        import ix_notebook_mcp.runtime as rt
        old_ns = rt._user_ns
        try:
            rt._user_ns = {"view": fake_mod}
            h = self.hint(exc)
            assert "view.grep" in h
            assert "signature" in h
        finally:
            rt._user_ns = old_ns

    def test_sh_extra_positional_arg_gets_argv_hint(self) -> None:
        """The common sh('git', 'status') mistake should point at sh([...])."""
        h = self.hint(TypeError("sh() takes 1 positional argument but 2 were given"))
        assert "sh(['git', 'status'])" in h
        assert "cwd=" in h

    def test_never_raises(self) -> None:
        """_type_error_hint must never raise regardless of input."""
        for exc in [
            TypeError(""),
            TypeError(None),  # type: ignore[arg-type]
            TypeError("a() missing 1 required keyword-only argument: 'x'"),
        ]:
            result = self.hint(exc)
            assert isinstance(result, str)
