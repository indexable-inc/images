{
  ix,
  lib,
  pkgs ? ix.pkgs,
}:

let
  # The PTY-driving `tui` package, baked into the pinned interpreter so every
  # session can `import tui` with no setup. The PyO3 cdylib comes from the same
  # shared workspace graph the binary is selected from, dropped next to the
  # package's Python source as the `tui._tui` extension. This is the cdylib
  # straight from the graph rather than the distributable wheel, so it also
  # works on macOS, where the wheel packaging stays Linux-only. Store references
  # in the cdylib are fine: this module never leaves the Nix environment.
  tuiPythonSource = builtins.path {
    name = "tui-py-python-source";
    path = ../tui-py/python;
  };
  tuiModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-tui-python-module"
      {
        strictDeps = true;
        propagatedBuildInputs = [ pkgs.python3.pkgs.numpy ];
        meta.description = "ix-tui PyO3 module bundled into the ix-mcp interpreter";
      }
      ''
        site="$out/${pkgs.python3.sitePackages}/tui"
        mkdir -p "$site"
        cp -r ${tuiPythonSource}/tui/. "$site/"

        cdylib=""
        for candidate in \
          ${ix.rustWorkspace.units.libraries.tui_py}/lib/libtui_py.so \
          ${ix.rustWorkspace.units.libraries.tui_py}/lib/libtui_py-*.so \
          ${ix.rustWorkspace.units.libraries.tui_py}/lib/libtui_py.dylib \
          ${ix.rustWorkspace.units.libraries.tui_py}/lib/libtui_py-*.dylib
        do
          if [ -f "$candidate" ]; then
            cdylib="$candidate"
            break
          fi
        done
        if [ -z "$cdylib" ]; then
          echo "ix-tui module: no cdylib under ${ix.rustWorkspace.units.libraries.tui_py}/lib" >&2
          ls -la ${ix.rustWorkspace.units.libraries.tui_py}/lib >&2 || true
          exit 1
        fi
        install -m555 "$cdylib" "$site/_tui.abi3.so"
      ''
  );

  # The search package, baked into the pinned interpreter so every
  # session can `import search` and `await search.semantic(...)`
  # with no setup. Same shape as `tuiModule`: the PyO3 cdylib comes from the
  # shared workspace graph (not the Linux-only wheel), so this also works on
  # macOS dev.
  searchPythonSource = builtins.path {
    name = "search-py-python-source";
    path = ../search-py/python;
  };
  searchModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-search-python-module"
      {
        strictDeps = true;
        meta.description = "ix-search PyO3 module bundled into the ix-mcp interpreter";
      }
      ''
        site="$out/${pkgs.python3.sitePackages}/search"
        mkdir -p "$site"
        cp -r ${searchPythonSource}/search/. "$site/"

        cdylib=""
        for candidate in \
          ${ix.rustWorkspace.units.libraries.search_py}/lib/libsearch_py.so \
          ${ix.rustWorkspace.units.libraries.search_py}/lib/libsearch_py-*.so \
          ${ix.rustWorkspace.units.libraries.search_py}/lib/libsearch_py.dylib \
          ${ix.rustWorkspace.units.libraries.search_py}/lib/libsearch_py-*.dylib
        do
          if [ -f "$candidate" ]; then
            cdylib="$candidate"
            break
          fi
        done
        if [ -z "$cdylib" ]; then
          echo "ix-search module: no cdylib under ${ix.rustWorkspace.units.libraries.search_py}/lib" >&2
          ls -la ${ix.rustWorkspace.units.libraries.search_py}/lib >&2 || true
          exit 1
        fi
        install -m555 "$cdylib" "$site/_search.abi3.so"
      ''
  );

  # The `fff` fast file-search package, baked into the interpreter so every
  # session can `import fff` and run fuzzy file search / SIMD grep over a repo
  # with no setup. Unlike `tui`/`search`, fff has no PyO3 binding: it ships a
  # stable C ABI (the `fff-c` cdylib, emitted next to `fff-mcp` by
  # `packages/fff`), and the pure-Python module loads it via ctypes. The cdylib
  # is dropped next to the package source so the module loads it by a fixed
  # path. Cross-platform: `pkgs.fff` builds on Linux and macOS. Store references
  # in the cdylib are fine: this module never leaves the Nix environment.
  fffPythonSource = builtins.path {
    name = "ix-mcp-fff-python-source";
    path = ./src/fff;
  };
  fffModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-fff-python-module"
      {
        strictDeps = true;
        meta.description = "fff fast file-search bound via ctypes, bundled into the ix-mcp interpreter";
      }
      ''
        site="$out/${pkgs.python3.sitePackages}/fff"
        mkdir -p "$site"
        cp -r ${fffPythonSource}/fff/. "$site/"

        cdylib=""
        for candidate in \
          ${pkgs.fff}/lib/libfff_c.so \
          ${pkgs.fff}/lib/libfff_c.dylib
        do
          if [ -f "$candidate" ]; then
            cdylib="$candidate"
            break
          fi
        done
        if [ -z "$cdylib" ]; then
          echo "ix-fff module: no libfff_c cdylib under ${pkgs.fff}/lib" >&2
          ls -la ${pkgs.fff}/lib >&2 || true
          exit 1
        fi
        install -m555 "$cdylib" "$site/$(basename "$cdylib")"
      ''
  );

  # The `ix_google` package: typed PyO3 bindings for the google-gmail and
  # google-calendar Rust crates, baked into the pinned interpreter as a
  # complement to the (untyped) `google_auth` helper. Notebook users pick
  # whichever fits: `import google_auth` gives the official googleapiclient
  # surface, `import ix_google` gives typed `gmail.Client()` /
  # `calendar.Client()` over the same shared OAuth grant. Auth bootstrap is
  # `gmail auth` or `gcal auth` on the host once.
  ixGooglePythonSource = builtins.path {
    name = "ix-google-python-source";
    path = ../google/py/python;
  };
  ixGoogleModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-google-python-module"
      {
        strictDeps = true;
        meta.description = "ix_google PyO3 module bundled into the ix-mcp interpreter";
      }
      ''
        site="$out/${pkgs.python3.sitePackages}/ix_google"
        mkdir -p "$site"
        cp -r ${ixGooglePythonSource}/ix_google/. "$site/"

        cdylib=""
        for candidate in \
          ${ix.rustWorkspace.units.libraries.ix_google_py}/lib/libix_google_py.so \
          ${ix.rustWorkspace.units.libraries.ix_google_py}/lib/libix_google_py-*.so \
          ${ix.rustWorkspace.units.libraries.ix_google_py}/lib/libix_google_py.dylib \
          ${ix.rustWorkspace.units.libraries.ix_google_py}/lib/libix_google_py-*.dylib
        do
          if [ -f "$candidate" ]; then
            cdylib="$candidate"
            break
          fi
        done
        if [ -z "$cdylib" ]; then
          echo "ix-google module: no cdylib under ${ix.rustWorkspace.units.libraries.ix_google_py}/lib" >&2
          ls -la ${ix.rustWorkspace.units.libraries.ix_google_py}/lib >&2 || true
          exit 1
        fi
        install -m555 "$cdylib" "$site/_ix_google.abi3.so"
      ''
  );

  # The single-tool MCP server itself, a pure-Python package installed into the
  # pinned interpreter so the `ix-mcp` entrypoint, the one shared kernel, and the
  # bundled modules all share one environment. No build step: plain Python over
  # ipykernel + jupyter-client + the bundled modules already in this interpreter.
  ixNotebookMcpSource = builtins.path {
    name = "ix-notebook-mcp-source";
    path = ./ix_notebook_mcp;
  };
  ixNotebookMcpModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-notebook-mcp-module"
      {
        strictDeps = true;
        meta.description = "The ix notebook-first MCP server package";
      }
      ''
        site="$out/${pkgs.python3.sitePackages}/ix_notebook_mcp"
        mkdir -p "$site"
        cp -r ${ixNotebookMcpSource}/. "$site/"
      ''
  );

  # `google_auth`: mint Google credentials for the bundled Gmail/Calendar Python
  # clients. Pure Python (no cdylib): it shells to the bundled `gcal` binary
  # (`IX_GCAL_BIN`, set on the wrapper below) for a short-lived access token from
  # the shared Google grant, and wraps it as a `google.oauth2.credentials`
  # object the official client accepts. The refresh token / client secret stay
  # inside `gcal`; only access tokens cross into Python.
  googleAuthPythonSource = builtins.path {
    name = "ix-mcp-google-auth-python-source";
    path = ./src/google_auth;
  };
  googleAuthModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-google-auth-python-module"
      {
        strictDeps = true;
        meta.description = "Google OAuth credentials helper bundled into the ix-mcp interpreter";
      }
      ''
        site="$out/${pkgs.python3.sitePackages}/google_auth"
        mkdir -p "$site"
        cp -r ${googleAuthPythonSource}/google_auth/. "$site/"
      ''
  );

  # Native macOS screen capture and cursor control, bundled like `tui` and
  # `search` so every session can `import screen`. This one is pure Python (no
  # PyO3 cdylib): it wraps the Apple-maintained pyobjc `Quartz` binding for
  # capture and synthetic input, and probes `AXIsProcessTrusted()` through
  # ctypes for the Accessibility (TCC) permission check. macOS-only: the module
  # itself raises on a non-Darwin platform, and `Quartz` is not available off
  # Darwin, so the dependency is gated below.
  screenPythonSource = builtins.path {
    name = "ix-mcp-screen-python-source";
    path = ./src/screen;
  };
  screenModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-screen-python-module"
      {
        strictDeps = true;
        meta.description = "Native macOS screen/cursor helper bundled into the ix-mcp interpreter";
      }
      ''
        site="$out/${pkgs.python3.sitePackages}/screen"
        mkdir -p "$site"
        cp -r ${screenPythonSource}/screen/. "$site/"
      ''
  );

  # Native macOS VM control, bundled like `screen` so every session can
  # `import vmkit` on Darwin. Pure Python: it spawns the `vmkit` binary (a
  # Rust binding over Virtualization.framework) and returns guest screenshots as
  # PIL images. macOS-only; on a non-Darwin platform the module raises.
  vmkitPythonSource = builtins.path {
    name = "ix-mcp-vmkit-python-source";
    path = ./src/vmkit;
  };
  vmkitModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-vmkit-python-module"
      {
        strictDeps = true;
        meta.description = "Native macOS VM control bundled into the ix-mcp interpreter";
      }
      ''
        site="$out/${pkgs.python3.sitePackages}/vmkit"
        mkdir -p "$site"
        cp -r ${vmkitPythonSource}/vmkit/. "$site/"
      ''
  );
  # The vmkit binary `vmkit` spawns. Darwin-only; referenced lazily so a Linux
  # mcp build never forces it.
  vmkitBin = ix.rustWorkspace.units.binaries."vmkit";

  # The gcal binary the calendar tools spawn with --json: the CLI surface of
  # the google-calendar crate (packages/google/calendar), so the MCP binding
  # carries no calendar logic of its own (RFC 0003).
  gcalBin = ix.rustWorkspace.units.binaries."gcal";

  # The `screen` helper is macOS-only, so its dependencies join the interpreter
  # only on Darwin. `pyobjc-framework-Quartz` is the maintained CoreGraphics
  # binding the helper wraps; Pillow (already transitive via matplotlib) carries
  # the PIL image type capture returns.
  darwinExtraPackages =
    ps:
    lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
      ps.pyobjc-framework-Quartz
      screenModule
      vmkitModule
    ];

  # The interpreter the wrapper pins. Sessions build their venv from this with
  # `--system-site-packages`, so `tui`, `search`, `fff`, `exa_py`, numpy, polars
  # (incl. Postgres via psycopg + SQLAlchemy), duckdb, httpx, and playwright are
  # importable by default while an in-session `pip install` still writes to the
  # per-session venv.
  mcpPython = pkgs.python3.withPackages (
    ps:
    [
      ps.asyncssh
      ps.numpy
      ps.polars
      # psycopg (v3) + SQLAlchemy so `polars.read_database` reaches Postgres out
      # of the box: `pl.read_database(sql, create_engine("postgresql+psycopg://…"))`.
      # connectorx and the ADBC drivers (what `read_database_uri` wants) are not
      # packaged in nixpkgs, so the SQLAlchemy-engine path is the supported one
      # here; psycopg also works as a raw DBAPI connection for `read_database`.
      ps.psycopg
      ps.sqlalchemy
      # duckdb: in-process analytical SQL over CSV/Parquet with no external
      # service; `duckdb.sql(q).pl()` hands results straight back to polars.
      # pyarrow is deliberately not bundled: it pulls arrow-cpp + grpc + the
      # aws/gcp/azure C++ SDKs (~300 MB) that this use case never touches, and
      # the polars/duckdb paths return frames natively. A session that needs
      # explicit Arrow tables (`pl.to_arrow()`) can `pip install pyarrow`.
      ps.duckdb
      # httpx: an HTTP client for the shared async loop (the session already speaks
      # async via asyncssh/playwright/tui but had no way to call a REST API). Sync
      # `httpx.get(...)` and `async with httpx.AsyncClient()` both work.
      ps.httpx
      # exa-py: the official Exa (exa.ai) SDK, so a session can run neural web
      # search, get page contents, and `answer(...)` over the live web with no
      # install step (`from exa_py import Exa`). It is a thin client over the Exa
      # REST API. No key is bundled: the caller brings `EXA_API_KEY` (sourced
      # from rbw/op per the secrets split), e.g. `Exa(os.environ["EXA_API_KEY"])`.
      ps.exa-py
      # Gmail / Google Workspace, the "third surface" for an integration alongside
      # the MCP binding and the index CLI (rfcs/0003): a session can drive the
      # Gmail and Calendar APIs directly with no install step. This is the official
      # client. Gmail is a Workspace API with no dedicated Cloud Client Library, so
      # google-api-python-client is the supported path (simplegmail rides on the
      # deprecated oauth2client with known token-refresh bugs). google-auth-oauthlib
      # carries the OAuth user-consent flow and google-auth-httplib2 the transport.
      # No credentials or tokens are bundled: the caller brings its own, sourced
      # from rbw/op per the secrets split.
      ps.google-api-python-client
      ps.google-auth-oauthlib
      ps.google-auth-httplib2
      # matplotlib (and Pillow, pulled in transitively) so plots and images are
      # capturable out of the box: the worker renders any open figure / object
      # with a `_repr_png_` back as an MCP image block.
      ps.matplotlib
      # playwright for browser automation out of the box. The Nix python package
      # is patched to use `playwright-driver` as its node driver, and the wrapper
      # below points PLAYWRIGHT_BROWSERS_PATH at the matching browser bundle, so
      # `from playwright.async_api import async_playwright` works with no
      # `playwright install` step. Driver and browsers are version-locked in
      # nixpkgs; keep them sourced from the same `playwright-driver` to stay in
      # sync. https://playwright.dev/python/docs/library
      ps.playwright
      # Execution engine: code runs on ONE real ipykernel on THIS interpreter
      # (driven over jupyter-client), so every bundled module (tui, search, the
      # data libraries) is importable with no install step.
      #   - ipykernel: the kernel the single shared session runs on.
      #   - jupyter-client: the client protocol the server drives it with.
      #   - nbformat: build the output dicts from kernel IOPub messages.
      #   - aiohttp: the tiny read-only dashboard over the execution store.
      #   - mcp: the Python MCP SDK that serves the tool surface over stdio/HTTP.
      ps.ipykernel
      ps.jupyter-client
      ps.nbformat
      ps.aiohttp
      ps.mcp
      tuiModule
      searchModule
      fffModule
      googleAuthModule
      ixGoogleModule
      ixNotebookMcpModule
    ]
    ++ darwinExtraPackages ps
  );

  # Browser bundle that matches the playwright-driver the python package is
  # patched to use. Exposed to the worker through PLAYWRIGHT_BROWSERS_PATH on the
  # wrapper below so launched browsers resolve without a network download.
  playwrightBrowsers = pkgs.playwright-driver.browsers;

  # `ix-mcp` is just the pinned interpreter invoked on the bundled package's CLI.
  # Everything (the entrypoint, the one shared kernel, the dashboard) runs in this
  # one interpreter, so the bundled modules are all importable with no install step.
  package =
    pkgs.runCommand "ix-mcp"
      {
        nativeBuildInputs = [ pkgs.makeWrapper ];
        strictDeps = true;
        meta = {
          description = "Notebook-first MCP server: an agent and a human co-edit one live Jupyter notebook";
          mainProgram = "ix-mcp";
        };
      }
      ''
        mkdir -p $out/bin
        makeWrapper ${lib.getExe mcpPython} $out/bin/ix-mcp \
          --add-flags "-m ix_notebook_mcp" \
          --set PLAYWRIGHT_BROWSERS_PATH ${lib.escapeShellArg playwrightBrowsers} \
          --set IX_GCAL_BIN ${lib.escapeShellArg "${gcalBin}/bin/gcal"} \
          ${lib.optionalString pkgs.stdenv.hostPlatform.isDarwin "--set IX_VMKIT_BIN ${lib.escapeShellArg "${vmkitBin}/bin/vmkit"}"}
      '';

  # Import a module in the pinned interpreter and assert a marker line. Used by
  # the bundled-module tests: the thing each guards is that the module is
  # importable in the very interpreter the kernels run on, which is a plain
  # interpreter import (no kernel, no network), so the build sandbox can prove it.
  importTest =
    name: code:
    pkgs.runCommand "ix-mcp-${name}"
      {
        nativeBuildInputs = [ mcpPython ];
        strictDeps = true;
      }
      ''
        ${lib.getExe mcpPython} -c ${lib.escapeShellArg code} >stdout 2>stderr || {
          echo "import test ${name} failed:" >&2
          cat stdout stderr >&2
          exit 1
        }
        grep -q '^${name}-ok' stdout || {
          echo "import test ${name} did not print its ok marker:" >&2
          cat stdout stderr >&2
          exit 1
        }
        mkdir -p "$out"
      '';

  tuiBundled = importTest "tui" "import tui; print('tui-ok', tui.__version__)";
  searchBundled = importTest "search" "import search; print('search-ok', search.__version__)";

  # End-to-end through the bundled `fff` ctypes module: index a temp tree, wait
  # for the scan, then prove fuzzy file search and content grep both return the
  # planted hits. Loads the fff-c cdylib from site-packages, so it also guards
  # that the library actually shipped next to the module. Pure local FS, no
  # network or watcher, so the build sandbox runs it.
  fffTestPy = pkgs.writeText "ix-mcp-fff-test.py" ''
    import os
    import tempfile
    import time

    import fff

    root = tempfile.mkdtemp()
    os.makedirs(os.path.join(root, "src"))
    with open(os.path.join(root, "hello_world.txt"), "w") as fh:
        fh.write("greetings\nfind me on this line\n")
    with open(os.path.join(root, "src", "main.rs"), "w") as fh:
        fh.write('fn main() {\n    println!("find me on this line");\n}\n')

    finder = fff.FileFinder(root, watch=False, content_indexing=True, ai_mode=True)
    try:
        # The initial scan runs in the background; poll until the planted file
        # is visible (a few short waits, robust to sandbox scheduling).
        hit_path = None
        for _ in range(20):
            finder.wait_for_scan(2000)
            result = finder.search("hello")
            match = next((h for h in result.items if "hello_world" in h.path), None)
            if match is not None:
                hit_path = match.path
                break
            time.sleep(0.25)
        assert hit_path is not None, f"fuzzy search did not find hello_world.txt: {result.items!r}"

        grep_result = finder.grep("find me on this line", limit=10)
        files = {m.path for m in grep_result.matches}
        assert any("hello_world" in f for f in files), f"grep missed the txt file: {files!r}"
        assert any("main.rs" in f for f in files), f"grep missed main.rs: {files!r}"
        defs = finder.grep("fn main", mode="regex", classify_definitions=True)
        assert defs.matches, "regex grep returned no matches"

        glob_result = finder.glob("**/*.rs")
        assert any("main.rs" in h.path for h in glob_result.items), (
            f"glob missed main.rs: {glob_result.items!r}"
        )
    finally:
        finder.close()

    print("fff-ok", fff.__version__)
  '';
  fffBundled =
    pkgs.runCommand "ix-mcp-fff"
      {
        nativeBuildInputs = [ mcpPython ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"
        ${lib.getExe mcpPython} ${fffTestPy} >stdout 2>stderr || {
          echo "ix-mcp fff test failed:" >&2
          cat stdout stderr >&2
          exit 1
        }
        grep -q '^fff-ok' stdout || {
          echo "ix-mcp fff test did not print its ok marker:" >&2
          cat stdout stderr >&2
          exit 1
        }
        mkdir -p "$out"
      '';
  dataLibsBundled = importTest "data-libs" (
    "import psycopg, sqlalchemy, duckdb, httpx; "
    + "from sqlalchemy import create_engine; create_engine('postgresql+psycopg://u@h/db'); "
    + "print('data-libs-ok')"
  );
  gmailLibsBundled = importTest "gmail-libs" (
    "from googleapiclient.discovery import build; from google.oauth2.credentials import Credentials; "
    + "import google_auth_oauthlib, google_auth_httplib2; "
    + "build('gmail', 'v1', credentials=Credentials(token='x'), static_discovery=True); "
    + "print('gmail-libs-ok')"
  );
  exaBundled = importTest "exa" (
    "from exa_py import Exa; e = Exa('dummy-key'); "
    + "assert callable(e.search) and callable(e.answer); "
    + "print('exa-ok')"
  );
  # The `google_auth` helper imports (pulling in google-auth) and exposes its
  # builders. A real token mint needs IX_GCAL_BIN + a prior `gcal auth`, so the
  # sandbox-safe assertion is the unset path: it must raise a clear, typed error
  # naming the missing piece rather than hanging or crashing vaguely.
  googleAuthBundled = importTest "google-auth" ''
    import os

    import google_auth

    assert callable(google_auth.credentials)
    assert callable(google_auth.gmail) and callable(google_auth.calendar)
    os.environ.pop("IX_GCAL_BIN", None)
    try:
        google_auth.credentials()
    except google_auth.GoogleAuthError as exc:
        assert "IX_GCAL_BIN" in str(exc), exc
    else:
        raise SystemExit("expected GoogleAuthError when IX_GCAL_BIN is unset")
    print("google-auth-ok")
  '';
  # Typed PyO3 bindings: the cdylib loads and the two Client classes are
  # callable. A real call would need GOOGLE_OAUTH_CLIENT_ID/SECRET and a
  # token file, so the sandbox-safe assertion is the import and the
  # class-shape check.
  ixGoogleBundled = importTest "ix-google" (
    "import ix_google; from ix_google import gmail, calendar; "
    + "assert callable(gmail.Client) and callable(calendar.Client); "
    + "print('ix-google-ok', ix_google.__version__)"
  );
  engineBundled = importTest "engine" "import ipykernel, jupyter_client, nbformat, aiohttp, mcp; print('engine-ok')";

  # The server package imports and registers its full tool surface. Exercises the
  # FastMCP registration (schemas from type hints) without starting a kernel or
  # the Jupyter Server, so it is sandbox-safe.
  serverTools = importTest "server" (
    "import asyncio; from ix_notebook_mcp.tools import mcp; "
    + "names = sorted(t.name for t in asyncio.run(mcp.list_tools())); "
    + "expected = {'python_exec','search_semantic','search_grep','calendar_events','calendar_event_create','calendar_event_cancel'}; "
    + "missing = expected - set(names); "
    + "assert not missing, ('missing tools: %r' % (missing,)); "
    + "print('server-ok', len(names))"
  );

  # End-to-end through the wrapper: run a real ipykernel and prove the historical
  # `ix-mcp eval` contract (`result:\n<repr>`) still holds. This is the one test
  # that boots a kernel (over loopback, which the sandbox allows), so it guards
  # the whole interpreter -> kernelspec -> execution path.
  evalSmoke =
    pkgs.runCommand "ix-mcp-eval-smoke"
      {
        nativeBuildInputs = [ package ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"

        ix-mcp eval '1 + 2' >stdout 2>stderr || {
          echo "ix-mcp eval failed:" >&2
          cat stdout stderr >&2
          exit 1
        }
        grep -qx 'result:' stdout && grep -qx '3' stdout || {
          echo "ix-mcp eval did not return the expected result:" >&2
          cat stdout stderr >&2
          exit 1
        }
        mkdir -p "$out"
      '';

  # Locks the bind-address default: with a working `tailscale status --json`
  # in PATH, `_tailscale_ip()` returns the first IPv4 from `Self.TailscaleIPs`
  # so the Jupyter Server is reachable from any tailnet peer; with no tailscale
  # binary, it returns None so the CLI falls through to loopback. The mock
  # binary lives in TMP/path so we control PATH exactly without touching the
  # real tailscale state. The mock is shell, not a subprocess of an actual
  # tailscale, so this test runs in the Nix sandbox.
  bindDefaultTest = pkgs.writeText "ix-mcp-bind-default-test.py" ''
    from unittest.mock import patch
    from ix_notebook_mcp import cli

    status = {
        "Self": {
            "TailscaleIPs": ["100.64.0.7", "fd7a::1"],
            "DNSName": "node.tail-x.ts.net.",
        }
    }

    # Happy path: tailscale is up. The helper picks the first IPv4 and strips
    # the trailing dot from the DNS name.
    with patch.object(cli, "_tailscale_status", return_value=status):
        assert cli._tailscale_ip() == "100.64.0.7", f"got {cli._tailscale_ip()!r}"
        assert cli._tailscale_dns_name() == "node.tail-x.ts.net", f"got {cli._tailscale_dns_name()!r}"

    # No tailscale: the helpers return None so the CLI falls back to loopback.
    # Stubbing the inner _tailscale_status is more robust than juggling PATH or
    # the absolute fallback paths the real helper probes (which exist on hydra
    # outside the sandbox, so a PATH-only test would still find them).
    with patch.object(cli, "_tailscale_status", return_value=None):
        assert cli._tailscale_ip() is None, "expected None when tailscale is unavailable"
        assert cli._tailscale_dns_name() is None, "expected None when tailscale is unavailable"

    # IPv6-only or empty IP list: still None (the bind expects IPv4).
    with patch.object(cli, "_tailscale_status", return_value={"Self": {"TailscaleIPs": ["fd7a::1"]}}):
        assert cli._tailscale_ip() is None, "IPv6-only TailscaleIPs should yield None"

    print("bind-default-ok")
  '';
  bindDefaultSmoke =
    pkgs.runCommand "ix-mcp-bind-default-smoke"
      {
        nativeBuildInputs = [ mcpPython ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"
        ${mcpPython}/bin/python3 ${bindDefaultTest} >stdout 2>stderr || {
          echo "ix-mcp bind-default smoke failed:" >&2
          cat stdout stderr >&2
          exit 1
        }
        grep -qx 'bind-default-ok' stdout || {
          echo "ix-mcp bind-default smoke did not confirm helper behaviour:" >&2
          cat stdout stderr >&2
          exit 1
        }
        mkdir -p "$out"
      '';

  # Exercises the in-kernel runtime (ix_notebook_mcp/runtime.py) in-process: two
  # jobs run concurrently on one event loop, neither blocks the other, each keeps
  # its own captured stdout, and the trailing expression is captured as the
  # result. This is the core "multiple async" contract, provable without a kernel
  # or network, so the sandbox runs it.
  runtimeTestPy = pkgs.writeText "ix-mcp-runtime-test.py" ''
    import asyncio

    from ix_notebook_mcp import runtime

    ns = {}
    runtime.install(ns)
    jobs = ns["jobs"]
    run = ns["__ix_run"]

    async def main():
        a = await run("import asyncio\nfor i in range(3):\n    print('A', i)\n    await asyncio.sleep(0.05)\n'A done'", budget=0.02, name="A")
        b = await run("import asyncio\nfor i in range(3):\n    print('B', i)\n    await asyncio.sleep(0.05)\n'B done'", budget=0.02, name="B")
        assert a.running() and b.running(), (a.status, b.status)
        assert len(jobs) == 2, len(jobs)
        await asyncio.sleep(0.5)
        assert a.status == "done" and b.status == "done", (a.status, b.status)
        assert "A 0" in a.output and "B 0" in b.output, (a.output, b.output)
        assert a.result == "A done" and b.result == "B done", (a.result, b.result)

    asyncio.run(main())
    print("runtime-ok")
  '';
  runtimeSmoke =
    pkgs.runCommand "ix-mcp-runtime-smoke"
      {
        nativeBuildInputs = [ mcpPython ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"
        ${lib.getExe mcpPython} ${runtimeTestPy} >stdout 2>stderr || {
          echo "ix-mcp runtime smoke failed:" >&2
          cat stdout stderr >&2
          exit 1
        }
        grep -qx 'runtime-ok' stdout || {
          echo "ix-mcp runtime smoke did not confirm concurrent jobs:" >&2
          cat stdout stderr >&2
          exit 1
        }
        mkdir -p "$out"
      '';

  # Exercises the rich-output capture path: a DataFrame result is persisted to the
  # store with its text/html bundle (so the dashboard renders a table, not a repr),
  # a display() call made while a job runs is captured the same way, and a bytes
  # image payload normalizes to a base64 string. Stands up an InteractiveShell
  # in-process so the formatter runs without booting a kernel; sandbox-safe.
  richTestPy = pkgs.writeText "ix-mcp-rich-test.py" ''
    import asyncio
    import json
    import os
    import sqlite3
    import tempfile

    from IPython.core.interactiveshell import InteractiveShell

    # A kernel always has a shell; this in-process test stands one up so the rich
    # formatter path runs without booting a kernel.
    InteractiveShell.instance()

    store_path = tempfile.mktemp(suffix=".db")
    os.environ["IX_MCP_STORE"] = store_path

    import polars as pl

    from ix_notebook_mcp import runtime

    # A bytes image payload must normalize to a base64 string: raw bytes would not
    # survive JSON storage or an <img> data URI.
    bundle = runtime._normalize_bundle({"image/png": b"\x89PNG\r\n", "text/plain": "x"})
    assert isinstance(bundle["data"]["image/png"], str), bundle

    ns = {"pl": pl}
    runtime.install(ns)
    run = ns["__ix_run"]


    async def main():
        # A DataFrame result is stored with its text/html bundle.
        df_job = await run("pl.DataFrame({'a': [1, 2], 'b': ['x', 'y']})", budget=3.0, name="df")
        await df_job.task
        conn = sqlite3.connect(store_path)
        conn.row_factory = sqlite3.Row
        row = conn.execute("SELECT status, outputs FROM executions WHERE id = ?", (df_job.id,)).fetchone()
        assert row["status"] == "done", row["status"]
        result_mimes = {mime for out in json.loads(row["outputs"]) for mime in out["data"]}
        assert "text/html" in result_mimes, ("result mimes", result_mimes)

        # A display() call made while a job runs is captured too.
        disp_job = await run(
            "from IPython.display import display\ndisplay(pl.DataFrame({'z': [9]}))",
            budget=3.0,
            name="disp",
        )
        await disp_job.task
        disp_outputs = conn.execute(
            "SELECT outputs FROM executions WHERE id = ?", (disp_job.id,)
        ).fetchone()[0]
        disp_mimes = {mime for out in json.loads(disp_outputs) for mime in out["data"]}
        assert "text/html" in disp_mimes, ("display mimes", disp_mimes)

        # A Result splits the human view (HTML on the dashboard) from the model
        # view (text in the tool result): the stored bundle carries user_html as
        # text/html, and to_mcp hands the model only the text/plain llm_result.
        from ix_notebook_mcp import outputs
        res_job = await run("Result(user_html='<b>hi</b>', llm_result='just-text')", budget=3.0, name="res")
        await res_job.task
        res_outputs = conn.execute("SELECT outputs FROM executions WHERE id = ?", (res_job.id,)).fetchone()[0]
        res_bundle = [out["data"] for out in json.loads(res_outputs)][-1]
        assert res_bundle.get("text/html") == "<b>hi</b>", res_bundle
        mcp = outputs.to_mcp([{"output_type": "execute_result", "data": res_bundle, "metadata": {}}])
        texts = [c.text for c in mcp if getattr(c, "text", None) is not None]
        assert texts == ["just-text"], texts


    asyncio.run(main())
    print("rich-ok")
  '';
  richSmoke =
    pkgs.runCommand "ix-mcp-rich-smoke"
      {
        nativeBuildInputs = [ mcpPython ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"
        ${lib.getExe mcpPython} ${richTestPy} >stdout 2>stderr || {
          echo "ix-mcp rich smoke failed:" >&2
          cat stdout stderr >&2
          exit 1
        }
        grep -qx 'rich-ok' stdout || {
          echo "ix-mcp rich smoke did not confirm rich-output capture:" >&2
          cat stdout stderr >&2
          exit 1
        }
        mkdir -p "$out"
      '';

  # The vmkit guest surfaces as a live dashboard resource: a booted Driver shows
  # up in Driver.list_all(), renders its framebuffer to inline-PNG HTML, and the
  # runtime's resource provider discovers it. Uses a fake proc + a seeded frame
  # so no real VM (or Virtualization.framework entitlement) is needed; pure
  # in-process, sandbox-safe.
  vmkitResourceTestPy = pkgs.writeText "ix-mcp-vmkit-resource-test.py" ''
    import asyncio
    import os
    import time
    import types

    import vmkit
    from ix_notebook_mcp import runtime

    d = vmkit.Driver(bundle="/tmp/guest.bundle")
    assert d.is_alive is False
    assert d.id and len(d.id) == 8
    assert d.title == "vm · guest.bundle", repr(d.title)
    # No frame yet, not booted: a placeholder, and no capture is attempted.
    assert "booting" in d.resource_html() and "<img" not in d.resource_html()

    # Pretend the guest is booted (poll() is None == running) and has a frame.
    d._proc = types.SimpleNamespace(poll=lambda: None)
    assert d.is_alive is True
    d._frame_png = b"\x89PNG\r\n\x1a\nFRAME"
    d._frame_at = time.time()
    html = d.resource_html()
    assert 'img src="data:image/png;base64,' in html, html[:120]

    vmkit.Driver._live[d.id] = d
    assert d in vmkit.Driver.list_all()

    # The runtime provider discovers it, keyed vm:<id>, kind "vm", and renders.
    runtime.resources.clear()
    runtime._discover_vmkit_resources()
    rid = "vm:" + d.id
    assert rid in runtime.resources, list(runtime.resources)
    res = runtime.resources[rid]
    assert res.kind == "vm" and res.alive() is True
    assert "<img" in asyncio.run(res.render_html())
    # Idempotent: a second sweep does not duplicate the resource.
    runtime._discover_vmkit_resources()
    assert sum(1 for k in runtime.resources if k == rid) == 1
    # The bounded grab read never holds the lockstep pipe forever, and a
    # timed-out ack is drained before the next command (no desync). Fake pipe:
    # stdin is a sink, stdout is a real os.pipe we feed.
    dd = vmkit.Driver(bundle="/tmp/g")
    rfd, wfd = os.pipe()
    rfile = os.fdopen(rfd, "r")
    wfile = os.fdopen(wfd, "w", buffering=1)

    class _Sink:
        def write(self, s):
            pass

        def flush(self):
            pass

    dd._proc = types.SimpleNamespace(
        poll=lambda: None, returncode=None, stdin=_Sink(), stdout=rfile
    )
    # Guest serial console can share stdout (Linux guest): non-ack lines are
    # skipped, the real ok/err ack is returned.
    wfile.write("[  OK  ] Reached target Initrd Root Device.\n")
    wfile.write("Starting File System Check...\n")
    wfile.write("\n")
    wfile.write("ok size 1280 800\n")
    assert dd._send_locked("size") == "ok size 1280 800"
    wfile.write("[  OK  ] Started Service.\n")
    wfile.write("err size guest framebuffer not available yet\n")
    try:
        dd._send_locked("size")
        raise AssertionError("err ack should raise")
    except vmkit.VmkitError as exc:
        assert "framebuffer not available" in str(exc), exc
    try:
        dd._send_locked("shot /tmp/x", ack_timeout=0.2)
        raise AssertionError("bounded read should have timed out")
    except vmkit.VmkitError as exc:
        assert "timed out" in str(exc), exc
    assert dd._pending_acks == 1
    wfile.write("ok\n")    # the late ack for the timed-out shot
    wfile.write("done\n")  # the next command's own ack
    wfile.flush()
    assert dd._send_locked("size") == "done"  # drained "ok", read its own ack
    assert dd._pending_acks == 0

    print("vmkit-resource-ok")
  '';
  vmkitResourceSmoke =
    pkgs.runCommand "ix-mcp-vmkit-resource-smoke"
      {
        nativeBuildInputs = [ mcpPython ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"
        ${lib.getExe mcpPython} ${vmkitResourceTestPy} >stdout 2>stderr || {
          echo "ix-mcp vmkit resource smoke failed:" >&2
          cat stdout stderr >&2
          exit 1
        }
        grep -qx 'vmkit-resource-ok' stdout || {
          echo "ix-mcp vmkit resource smoke did not confirm the resource path:" >&2
          cat stdout stderr >&2
          exit 1
        }
        mkdir -p "$out"
      '';

  # macOS-only modules (`screen`, `vmkit`) are only bundled on Darwin; their
  # import tests only exist there.
  screenBundled = importTest "screen" "import screen; print('screen-ok', callable(screen.capture), callable(screen.click), callable(screen.accessibility_trusted))";
  vmkitBundled = importTest "vmkit" "import vmkit; print('vmkit-ok', callable(vmkit.boot_linux), callable(vmkit.drive), callable(vmkit.screenshot))";
in
package.overrideAttrs (old: {
  passthru = (old.passthru or { }) // {
    tests = {
      inherit
        tuiBundled
        searchBundled
        fffBundled
        dataLibsBundled
        gmailLibsBundled
        exaBundled
        googleAuthBundled
        ixGoogleBundled
        engineBundled
        serverTools
        evalSmoke
        runtimeSmoke
        richSmoke
        bindDefaultSmoke
        ;
    }
    // lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
      inherit screenBundled vmkitBundled vmkitResourceSmoke;
    };
  };
})
