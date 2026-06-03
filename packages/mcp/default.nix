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

  # The notebook-first MCP server itself, a pure-Python package installed into
  # the pinned interpreter so the `ix-mcp` entrypoint, the Jupyter Server
  # extension, and the kernels all share one environment (that co-location is
  # what makes real-time co-editing work). No build step: it is plain Python that
  # imports the Jupyter stack and bundled modules already in this interpreter.
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
  # `import macvm` on Darwin. Pure Python: it spawns the `macos-vm` binary (a
  # Rust binding over Virtualization.framework) and returns guest screenshots as
  # PIL images. macOS-only; on a non-Darwin platform the module raises.
  macvmPythonSource = builtins.path {
    name = "ix-mcp-macvm-python-source";
    path = ./src/macvm;
  };
  macvmModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-macvm-python-module"
      {
        strictDeps = true;
        meta.description = "Native macOS VM control bundled into the ix-mcp interpreter";
      }
      ''
        site="$out/${pkgs.python3.sitePackages}/macvm"
        mkdir -p "$site"
        cp -r ${macvmPythonSource}/macvm/. "$site/"
      ''
  );
  # The macos-vm binary `macvm` spawns. Darwin-only; referenced lazily so a Linux
  # mcp build never forces it.
  macosVmBin = ix.rustWorkspace.units.binaries."macos-vm";

  # The `screen` helper is macOS-only, so its dependencies join the interpreter
  # only on Darwin. `pyobjc-framework-Quartz` is the maintained CoreGraphics
  # binding the helper wraps; Pillow (already transitive via matplotlib) carries
  # the PIL image type capture returns.
  darwinExtraPackages =
    ps:
    lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
      ps.pyobjc-framework-Quartz
      screenModule
      macvmModule
    ];

  # The interpreter the wrapper pins. Sessions build their venv from this with
  # `--system-site-packages`, so `tui`, `search`, numpy, polars (incl. Postgres
  # via psycopg + SQLAlchemy), duckdb, httpx, and playwright are importable by
  # default while an in-session `pip install` still writes to the per-session
  # venv.
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
      # Jupyter stack: the notebook-first MCP server drives a real Jupyter
      # Server with real-time collaboration, and code runs on a real ipykernel
      # on THIS interpreter, so every bundled module (tui, search, the data
      # libraries) is importable inside notebook cells with no install step.
      #   - ipykernel + jupyter-client: the kernel and the client protocol.
      #   - jupyter-server + jupyterlab + notebook: the server a human opens to
      #     co-edit, and the kernel/session manager the MCP tools drive.
      #   - nbformat/nbclient/nbconvert: build and serialize .ipynb outputs.
      #   - jupyter-collaboration (+ -ui, -docprovider, -server-ydoc, -ydoc) and
      #     pycrdt(-websocket): the Yjs/CRDT layer that lets the agent and a
      #     human edit and execute ONE live notebook at the same time. MCP writes
      #     go through this layer (not direct file writes) so the browser never
      #     desyncs.
      #   - mcp: the Python MCP SDK that serves the tool surface over stdio/HTTP.
      ps.ipykernel
      ps.jupyter-client
      ps.jupyter-server
      ps.jupyterlab
      ps.notebook
      ps.nbformat
      ps.nbclient
      ps.nbconvert
      ps.jupyter-collaboration
      ps.jupyter-collaboration-ui
      ps.jupyter-docprovider
      ps.jupyter-server-ydoc
      ps.jupyter-ydoc
      ps.pycrdt
      ps.pycrdt-websocket
      ps.mcp
      tuiModule
      searchModule
      ixNotebookMcpModule
    ]
    ++ darwinExtraPackages ps
  );

  # Browser bundle that matches the playwright-driver the python package is
  # patched to use. Exposed to the worker through PLAYWRIGHT_BROWSERS_PATH on the
  # wrapper below so launched browsers resolve without a network download.
  playwrightBrowsers = pkgs.playwright-driver.browsers;

  # `ix-mcp` is just the pinned interpreter invoked on the bundled package's CLI.
  # Everything (the entrypoint, the Jupyter Server extension it launches, and the
  # notebook kernels) runs in this one interpreter, so the bundled modules and
  # the Jupyter stack are all importable with no install step.
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
          ${lib.optionalString pkgs.stdenv.hostPlatform.isDarwin "--set IX_MACVM_BIN ${lib.escapeShellArg "${macosVmBin}/bin/macos-vm"}"}
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
  jupyterBundled = importTest "jupyter" (
    "import ipykernel, jupyter_server, jupyterlab, nbformat, jupyter_collaboration, "
    + "jupyter_server_ydoc, jupyter_ydoc, pycrdt, mcp; "
    + "print('jupyter-ok')"
  );

  # The server package imports and registers its full tool surface. Exercises the
  # FastMCP registration (schemas from type hints) without starting a kernel or
  # the Jupyter Server, so it is sandbox-safe.
  serverTools = importTest "server" (
    "import asyncio; from ix_notebook_mcp.tools import mcp; "
    + "names = sorted(t.name for t in asyncio.run(mcp.list_tools())); "
    + "expected = {'notebook_use','notebook_read','cell_add','cell_run','cell_overwrite','cell_delete','run_code','kernel_restart','search_semantic','search_grep'}; "
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

  # macOS-only modules (`screen`, `macvm`) are only bundled on Darwin; their
  # import tests only exist there.
  screenBundled = importTest "screen" "import screen; print('screen-ok', callable(screen.capture), callable(screen.click), callable(screen.accessibility_trusted))";
  macvmBundled = importTest "macvm" "import macvm; print('macvm-ok', callable(macvm.boot_linux), callable(macvm.drive), callable(macvm.screenshot))";
in
package.overrideAttrs (old: {
  passthru = (old.passthru or { }) // {
    tests = {
      inherit
        tuiBundled
        searchBundled
        dataLibsBundled
        gmailLibsBundled
        jupyterBundled
        serverTools
        evalSmoke
        ;
    }
    // lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
      inherit screenBundled macvmBundled;
    };
  };
})
