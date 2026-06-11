"""Regression tests for ENG-2490.

1. fff.grep / agrep / map / amap all accept calls without an explicit mode
   (mode defaults to 'plain').
2. _type_error_hint appends a live-signature hint to call-binding TypeErrors
   and returns '' for non-binding TypeErrors or non-TypeError exceptions.
"""
from __future__ import annotations

import inspect
import re
import shutil
import sys
import tempfile
from pathlib import Path

import pytest


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# Add the fff source dir to sys.path and copy the shared library next to it
# so the module can import without a full nix environment.
FFF_SRC = Path(__file__).parent / "src" / "fff"
FFF_SO_SRC = Path(
    "/nix/store/l53nqagn78xqpnpnph3za2f0rm6nsgif-ix-mcp-fff-python-module"
    "/lib/python3.13/site-packages/fff/libfff_c.so"
)
FFF_SO_DST = FFF_SRC / "fff" / "libfff_c.so"


def _ensure_so() -> bool:
    """Copy the shared library if needed; return True if available."""
    if FFF_SO_DST.exists():
        return True
    if not FFF_SO_SRC.exists():
        return False
    shutil.copy2(FFF_SO_SRC, FFF_SO_DST)
    return True


_SO_AVAILABLE = _ensure_so()

if str(FFF_SRC) not in sys.path:
    sys.path.insert(0, str(FFF_SRC))


# ---------------------------------------------------------------------------
# Part 1: fff signature defaults
# ---------------------------------------------------------------------------

class TestFffGrepSignatureDefaults:
    """mode='plain' is now the default for grep, agrep, map, amap."""

    def _import_fff(self):
        import importlib
        import fff
        importlib.reload(fff)
        return fff

    @pytest.mark.skipif(not _SO_AVAILABLE, reason="libfff_c.so not available")
    def test_grep_mode_has_default(self):
        fff = self._import_fff()
        sig = inspect.signature(fff.grep)
        p = sig.parameters["mode"]
        assert p.default == "plain", f"expected default 'plain', got {p.default!r}"

    @pytest.mark.skipif(not _SO_AVAILABLE, reason="libfff_c.so not available")
    def test_agrep_mode_has_default(self):
        fff = self._import_fff()
        sig = inspect.signature(fff.agrep)
        p = sig.parameters["mode"]
        assert p.default == "plain"

    @pytest.mark.skipif(not _SO_AVAILABLE, reason="libfff_c.so not available")
    def test_map_mode_has_default(self):
        fff = self._import_fff()
        sig = inspect.signature(fff.map)
        p = sig.parameters["mode"]
        assert p.default == "plain"

    @pytest.mark.skipif(not _SO_AVAILABLE, reason="libfff_c.so not available")
    def test_amap_mode_has_default(self):
        fff = self._import_fff()
        sig = inspect.signature(fff.amap)
        p = sig.parameters["mode"]
        assert p.default == "plain"

    @pytest.mark.skipif(not _SO_AVAILABLE, reason="libfff_c.so not available")
    def test_grep_works_without_mode(self):
        """fff.grep(query, path) should work without passing mode."""
        fff = self._import_fff()
        with tempfile.TemporaryDirectory() as tmp:
            # Write a file with known content.
            p = Path(tmp) / "hello.txt"
            p.write_text("hello world\n")
            result = fff.grep("hello", tmp)
            assert result is not None

    @pytest.mark.skipif(not _SO_AVAILABLE, reason="libfff_c.so not available")
    def test_grep_max_results_raises_typeerror(self):
        """The ticket's exact repro: max_results= is still not a valid kwarg."""
        fff = self._import_fff()
        with tempfile.TemporaryDirectory() as tmp:
            with pytest.raises(TypeError, match="max_results"):
                fff.grep("quic-ingress", tmp, max_results=20)


# ---------------------------------------------------------------------------
# Part 2: _type_error_hint in runtime.py
# ---------------------------------------------------------------------------

# Import the helper directly (no kernel setup needed).
sys.path.insert(0, str(Path(__file__).parent))


def _get_hint_fn():
    """Import _type_error_hint; skip if the module itself can't be imported."""
    from ix_notebook_mcp.runtime import _type_error_hint
    return _type_error_hint


# Module-level keyword-only function used as a test target for TypeError hints.
# Must be at module level so Python names it "kw_target" (not "Class.kw_target")
# in TypeError messages, matching the regex used by _type_error_hint.
def kw_target(*, query: str, mode: str = "plain", limit: int = 50):
    pass


def _kw_binding_error(*args, **kwargs) -> TypeError:
    try:
        kw_target(*args, **kwargs)
    except TypeError as e:
        return e
    raise AssertionError("expected TypeError")


class TestTypeErrorHint:
    """_type_error_hint appends a live signature hint to call-binding errors."""

    def setup_method(self):
        self.hint = _get_hint_fn()

    def test_unexpected_keyword_arg(self):
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

    def test_missing_required_kwarg(self):
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

    def test_non_binding_typeerror_no_hint(self):
        """A TypeError raised inside a function body should get no hint."""
        exc = TypeError("cannot add str and int")
        h = self.hint(exc)
        assert h == ""

    def test_unknown_callable_no_hint(self):
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

    def test_module_qualified_lookup(self):
        """When the function lives on a module in the namespace (e.g. fff.grep),
        the hint should find it via the module."""
        import types
        fake_mod = types.SimpleNamespace()
        fake_mod.grep = kw_target  # simulate fff.grep

        exc = TypeError("grep() got an unexpected keyword argument 'max_results'")
        import ix_notebook_mcp.runtime as rt
        old_ns = rt._user_ns
        try:
            rt._user_ns = {"fff": fake_mod}
            h = self.hint(exc)
            assert "fff.grep" in h
            assert "signature" in h
        finally:
            rt._user_ns = old_ns

    def test_never_raises(self):
        """_type_error_hint must never raise regardless of input."""
        for exc in [
            TypeError(""),
            TypeError(None),  # type: ignore[arg-type]
            TypeError("a() missing 1 required keyword-only argument: 'x'"),
        ]:
            result = self.hint(exc)
            assert isinstance(result, str)
