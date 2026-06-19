"""Lazy auto-binding of bundled modules.

Every bundled module is bound into the kernel namespace so it is usable with no
``import`` (the way ``fff``/``view`` are), but the framework-heavy and
platform-specific ones (``maps`` pulls in MapKit + CoreLocation, ~120ms; macOS-only
modules are absent on Linux) must not pay that cost at startup. ``runtime._LazyModule``
is the deferral: an untouched proxy costs nothing, the first *public* attribute
access imports the real module, an underscore/introspection probe never imports, and
an absent module raises an ordinary ImportError only when first used. The proxies are
bound OUT of ``_baseline_names`` and excluded from the checkpoint / namespace pane by
type, so a user variable that shadows a module name (e.g. a temp ``x``) stays real
user state. These tests pin that contract.
"""

from __future__ import annotations

import types

from ix_notebook_mcp import registry, runtime

_MISSING = "ix_definitely_not_a_real_module_xyz"


def test_proxy_construction_does_not_import() -> None:
    # Binding a proxy must be free: no import of a missing module, no error.
    assert isinstance(runtime._LazyModule(_MISSING), runtime._LazyModule)


def test_repr_does_not_trigger_import() -> None:
    # The dashboard / repr must be safe to call on an untouched proxy.
    r = repr(runtime._LazyModule(_MISSING))
    assert _MISSING in r
    assert "lazy" in r.lower()


def test_public_access_imports_and_delegates_to_the_real_module() -> None:
    # Use a stdlib module as a stand-in for a bundled one: same import machinery.
    proxy = runtime._LazyModule("json")
    assert proxy.dumps({"a": 1}) == '{"a": 1}'  # delegates to the real module
    import json as real_json

    assert proxy.dumps is real_json.dumps
    # No swap: the proxy stays a proxy (re-access is ~free via sys.modules).
    assert isinstance(proxy, runtime._LazyModule)


def test_missing_module_defers_error_to_first_public_use() -> None:
    proxy = runtime._LazyModule(_MISSING)
    raised = False
    try:
        _ = proxy.anything
    except ImportError:
        raised = True
    assert raised, "a lazy proxy over an absent module must raise ImportError on first use"


def test_underscore_and_introspection_probes_never_import() -> None:
    # pickle (__reduce_ex__ exists on object, so it never reaches __getattr__),
    # IPython display (_repr_html_), and hasattr() checks must not import -- even
    # for a platform-absent module, where importing would raise spuriously.
    proxy = runtime._LazyModule(_MISSING)
    assert hasattr(proxy, "__reduce_ex__")  # inherited from object, no import
    assert not hasattr(proxy, "_repr_html_")  # absent + underscore -> AttributeError, no import
    assert not hasattr(proxy, "_ipython_canary_method_should_not_exist_")
    raised_attr = False
    try:
        _ = proxy._private
    except AttributeError:
        raised_attr = True
    assert raised_attr, "underscore access must raise AttributeError, not import"


def test_registry_marks_only_the_cheap_modules_eager() -> None:
    # The eager set stays the cheap, always-loaded module (view) so startup is not
    # taxed by heavy modules; everything else (incl. maps) is bound lazily. (The
    # fsearch search helpers grep/find/spotlight are bound as top-level builtins,
    # not preimport modules.)
    eager = set(registry.preimport_names())
    every = set(registry.module_names())
    assert eager <= every
    assert eager == {"view"}, eager
    assert "maps" in every
    assert "maps" not in eager


def test_proxies_are_not_baseline_so_shadowing_user_vars_survive() -> None:
    # C1: an untouched proxy is dropped from the checkpoint / namespace pane by
    # TYPE, but a user variable that shadows a bundled-module name (a temp `x`, the
    # Twitter module's name) must remain real user state.
    saved_b, saved_l = runtime._baseline_names, runtime._lazy_module_names
    try:
        runtime._baseline_names = frozenset({"Result"})
        runtime._lazy_module_names = frozenset({"maps", "x"})
        proxy = runtime._LazyModule("maps")
        # Untouched proxy: excluded from both the checkpoint and the namespace pane.
        assert "maps" not in runtime._snapshot_candidates({"maps": proxy})
        assert "maps" not in runtime._namespace_candidates({"maps": proxy})
        # User var shadowing a module name: kept by both.
        assert runtime._snapshot_candidates({"x": 5}) == {"x": 5}
        assert "x" in runtime._namespace_candidates({"x": 5})
    finally:
        runtime._baseline_names, runtime._lazy_module_names = saved_b, saved_l


def test_install_binds_proxies_outside_baseline_and_seeds_them() -> None:
    # End-to-end against the real install(): maps is bound (lazily), kept out of the
    # baseline (so user shadowing survives), recorded for per-session seeding, and
    # NOT eager-imported at startup.
    import sys

    ns: dict = {}
    runtime.install(ns)
    assert isinstance(ns.get("maps"), runtime._LazyModule)
    assert "maps" not in runtime._baseline_names
    assert "maps" in runtime._lazy_module_names
    assert "maps" not in sys.modules, "install() must not eager-import maps"
    assert not isinstance(ns.get("view"), runtime._LazyModule)  # view stays eager
    assert isinstance(ns.get("view"), types.ModuleType)
    # The fsearch search helpers are bound eagerly as top-level callables.
    assert callable(ns.get("grep")) and callable(ns.get("find")) and callable(ns.get("spotlight"))
    # A fresh per-session namespace is seeded with the proxy too.
    sess = runtime._session_ns("sess-lazy-test")
    assert isinstance(sess.get("maps"), runtime._LazyModule)


def test_new_session_gets_a_fresh_proxy_not_shared_user_state() -> None:
    # A no-session cell (or a restored checkpoint) may rebind a lazy-module name in
    # the shared namespace (e.g. `x = 5`, x being the Twitter module's name). A fresh
    # session must NOT inherit that user value -- it gets its own lazy proxy.
    saved = (
        runtime._user_ns,
        runtime._baseline_names,
        runtime._lazy_module_names,
        dict(runtime._session_namespaces),
    )
    try:
        runtime._user_ns = {"Result": object(), "x": 5}  # shared dict holds user x=5
        runtime._baseline_names = frozenset({"Result"})
        runtime._lazy_module_names = frozenset({"x"})
        runtime._session_namespaces.clear()
        sess = runtime._session_ns("leak-test")
        assert isinstance(sess["x"], runtime._LazyModule)  # the proxy, not 5
        assert sess["x"] is not runtime._user_ns["x"]
    finally:
        runtime._user_ns, runtime._baseline_names, runtime._lazy_module_names, prev = saved
        runtime._session_namespaces.clear()
        runtime._session_namespaces.update(prev)


if __name__ == "__main__":
    fns = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    for fn in fns:
        fn()
    print(f"{len(fns)} passed")
