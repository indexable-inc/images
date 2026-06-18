"""Device-free tests for the `iphone` helper.

These never touch a real device: they check the module's shape (exports, async
signatures, explicit type hints), that the CLI path resolver always yields a
usable argv, that `start_tunneld` refuses to escalate without an explicit opt-in,
and that a missing device produces a clear error.
"""
from __future__ import annotations

import asyncio
import inspect
import sys
from pathlib import Path

import pytest

# Prefer the bundled module (nix check); fall back to the source tree (dev run).
IPHONE_SRC = Path(__file__).resolve().parents[1] / "src" / "iphone"
if IPHONE_SRC.is_dir() and str(IPHONE_SRC) not in sys.path:
    sys.path.insert(0, str(IPHONE_SRC))

import iphone

# Public callables = everything exported except the error class.
_PUBLIC_FUNCS = [
    getattr(iphone, name)
    for name in iphone.__all__
    if name != "IphoneError"
]


def test_all_names_exist() -> None:
    for name in iphone.__all__:
        assert hasattr(iphone, name), f"{name} in __all__ but missing from module"


def test_error_type() -> None:
    assert issubclass(iphone.IphoneError, RuntimeError)


def test_public_funcs_are_async() -> None:
    for func in _PUBLIC_FUNCS:
        assert inspect.iscoroutinefunction(func), f"{func.__name__} should be async"


def test_type_hints_explicit() -> None:
    # Mirrors the ruff ANN gate: every public function fully annotates its params
    # and return type.
    for func in _PUBLIC_FUNCS:
        sig = inspect.signature(func)
        assert sig.return_annotation is not inspect.Signature.empty, (
            f"{func.__name__} missing return annotation"
        )
        for param in sig.parameters.values():
            assert param.annotation is not inspect.Parameter.empty, (
                f"{func.__name__}({param.name}) missing annotation"
            )


def test_pmd3_argv_resolves() -> None:
    argv = iphone._pmd3_argv()
    assert isinstance(argv, list)
    assert argv, "argv must be a non-empty list"
    # Either the located executable, or the `python -m pymobiledevice3` fallback.
    assert argv[-1] == "pymobiledevice3" or argv[0].endswith("pymobiledevice3"), argv


def test_start_tunneld_requires_sudo() -> None:
    # Without sudo=True it must raise before spawning anything.
    with pytest.raises(iphone.IphoneError, match="sudo"):
        asyncio.run(iphone.start_tunneld())


def test_no_device_message(monkeypatch: pytest.MonkeyPatch) -> None:
    import polars as pl

    async def _empty() -> pl.DataFrame:
        return pl.DataFrame({"UniqueDeviceID": []})

    monkeypatch.setattr(iphone, "devices", _empty)
    with pytest.raises(iphone.IphoneError, match="no device connected"):
        asyncio.run(iphone._one_device())


def test_tap_is_coordinate_based() -> None:
    # tap moved from a (broken) WDA selector to W3C coordinate taps.
    params = list(inspect.signature(iphone.tap).parameters)
    assert params[:2] == ["x", "y"], params


def test_wda_down_raises_cleanly(monkeypatch: pytest.MonkeyPatch) -> None:
    # A connection error must surface as a clear IphoneError, and _wda_up must
    # report False rather than raising. Monkeypatch urlopen so the test is
    # hermetic (the macOS nix sandbox does not isolate loopback, so a real port
    # probe could reach a host WDA forward).
    import urllib.request

    def _refuse(*_a: object, **_k: object) -> object:
        raise OSError("connection refused")

    monkeypatch.setattr(urllib.request, "urlopen", _refuse)
    assert asyncio.run(iphone._wda_up()) is False
    with pytest.raises(iphone.IphoneError, match="WDA"):
        asyncio.run(iphone._wda("GET", "/status"))


def test_session_heals_on_expiry(monkeypatch: pytest.MonkeyPatch) -> None:
    # A stale session must be dropped and re-minted once on a session error.
    calls: list[str] = []

    async def fake_wda(method: str, path: str, body: object = None) -> object:
        calls.append(path)
        if path.startswith("/session/stale"):
            raise iphone.IphoneError("invalid session id")
        if path == "/session":
            return {"sessionId": "fresh"}
        return {}

    monkeypatch.setattr(iphone, "_wda_session", "stale")
    monkeypatch.setattr(iphone, "_wda", fake_wda)
    asyncio.run(iphone._wda_in_session("POST", "/actions", {}))
    assert any(c == "/session/fresh/actions" for c in calls), calls


def test_doctor_never_raises() -> None:
    # doctor() reports each check; it must not raise even with no device/WDA.
    frame = asyncio.run(iphone.doctor())
    assert "check" in frame.columns
    assert "ok" in frame.columns
