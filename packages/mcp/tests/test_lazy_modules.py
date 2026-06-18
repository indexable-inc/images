"""Lazy auto-binding of bundled modules.

Every bundled module is bound into the kernel namespace so it is usable with no
``import`` (the way ``fff``/``view`` are), but the framework-heavy and
platform-specific ones (``maps`` pulls in MapKit + CoreLocation, ~120ms; macOS-only
modules are absent on Linux) must not pay that cost at startup. ``runtime._LazyModule``
is the deferral: an untouched proxy costs nothing, the first attribute access imports
the real module and swaps itself out of the namespace, and an absent module raises an
ordinary ImportError only when first used. These tests pin that contract.
"""

from __future__ import annotations

import types

from ix_notebook_mcp import registry, runtime

_MISSING = "ix_definitely_not_a_real_module_xyz"


def test_proxy_construction_does_not_import() -> None:
    # Binding a proxy must be free: no import of a missing module, no error.
    ns: dict = {}
    ns["m"] = runtime._LazyModule(_MISSING, ns)
    assert isinstance(ns["m"], runtime._LazyModule)


def test_repr_does_not_trigger_import() -> None:
    # The dashboard / repr must be safe to call on an untouched proxy.
    proxy = runtime._LazyModule(_MISSING, {})
    r = repr(proxy)
    assert _MISSING in r and "lazy" in r.lower()


def test_first_access_imports_and_swaps_in_the_real_module() -> None:
    # Use a stdlib module as a stand-in for a bundled one: same import machinery.
    ns: dict = {}
    ns["json"] = runtime._LazyModule("json", ns)
    # Touching an attribute resolves the real module and uses it...
    assert ns["json"].dumps({"a": 1}) == '{"a": 1}'
    # ...and the proxy has replaced itself with the genuine module in the namespace.
    import json as real_json

    assert ns["json"] is real_json
    assert isinstance(ns["json"], types.ModuleType)


def test_missing_module_defers_error_to_first_use() -> None:
    proxy = runtime._LazyModule(_MISSING, {})
    raised = False
    try:
        _ = proxy.anything
    except ImportError:
        raised = True
    assert raised, "a lazy proxy over an absent module must raise ImportError on first use"


def test_registry_marks_only_the_cheap_modules_eager() -> None:
    # The lazy set is everything that is not pre-imported; the eager set stays the
    # two cheap, always-loaded ones so startup is not taxed by heavy modules.
    eager = set(registry.preimport_names())
    every = set(registry.module_names())
    assert eager <= every
    assert eager == {"fff", "view"}, eager
    # maps is the module that motivated this: present, and lazily bound (not eager).
    assert "maps" in every and "maps" not in eager


if __name__ == "__main__":
    fns = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    for fn in fns:
        fn()
    print(f"{len(fns)} passed")
