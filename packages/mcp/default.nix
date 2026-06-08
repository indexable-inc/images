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
  # Pretty, composable views of files and search results (view.ls/tree/grep/find
  # return polars DataFrames; view.cat/json/diff return highlighted Code). Pure
  # Python over the bundled fff/polars/pygments; cross-platform, so every session
  # can `import view`.
  viewPythonSource = builtins.path {
    name = "ix-mcp-view-python-source";
    path = ./src/view;
  };
  viewModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-view-python-module"
      {
        strictDeps = true;
        meta.description = "Pretty composable file/search views bundled into the ix-mcp interpreter";
      }
      ''
        site="$out/${pkgs.python3.sitePackages}/view"
        mkdir -p "$site"
        cp -r ${viewPythonSource}/view/. "$site/"
      ''
  );

  # `nix`: parse a `nix --log-format internal-json` stream into polars frames (a
  # durable event log + a folded build-DAG view) and a live, self-closing
  # dashboard Resource. Pure Python over the bundled polars (+ the runtime's
  # register_resource when in a kernel), cross-platform, so every session can
  # `import nix` and `await nix.build(".#foo")`.
  nixPythonSource = builtins.path {
    name = "ix-mcp-nix-python-source";
    path = ./src/nix;
  };
  nixModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-nix-python-module"
      {
        strictDeps = true;
        meta.description = "nix internal-json -> polars + live build-DAG, bundled into the ix-mcp interpreter";
      }
      ''
        site="$out/${pkgs.python3.sitePackages}/nix"
        mkdir -p "$site"
        cp -r ${nixPythonSource}/nix/. "$site/"
      ''
  );
  # Polars-returning SSH fan-out source: `import fleet`, then `await fleet.scan`
  # runs a command on many hosts in parallel (asyncssh + a bounded semaphore on
  # the shared loop) and combines per-host stdout into one DataFrame via
  # `pl.concat(how="diagonal_relaxed")`. Pure Python over the bundled asyncssh +
  # polars; cross-platform, so every session can `import fleet`.
  fleetPythonSource = builtins.path {
    name = "ix-mcp-fleet-python-source";
    path = ./src/fleet;
  };
  fleetModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-fleet-python-module"
      {
        strictDeps = true;
        meta.description = "Polars-returning SSH fan-out source bundled into the ix-mcp interpreter";
      }
      ''
        site="$out/${pkgs.python3.sitePackages}/fleet"
        mkdir -p "$site"
        cp -r ${fleetPythonSource}/fleet/. "$site/"
      ''
  );
  # Async shell-out helper: `import sh`, then `out = await sh("gh run list")`.
  # Runs on the kernel's loop (never blocks it like a bare subprocess.run) and
  # returns an Output that IS a Result, so the dashboard sees the command's ANSI
  # color rendered to HTML while the model gets the same text escape-stripped.
  # Pure Python over the bundled ansi2html; cross-platform.
  shPythonSource = builtins.path {
    name = "ix-mcp-sh-python-source";
    path = ./src/sh;
  };
  shModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-sh-python-module"
      {
        strictDeps = true;
        meta.description = "Async shell-out helper bundled into the ix-mcp interpreter";
      }
      ''
        site="$out/${pkgs.python3.sitePackages}/sh"
        mkdir -p "$site"
        cp -r ${shPythonSource}/sh/. "$site/"
      ''
  );
  # Git worktrees as the unit of isolated work: `import worktree`, then
  # `wt = await worktree.add("my-fix")` checks out a new branch in its own tree,
  # `await wt.build(".#mcp")` stages + nix-builds it, `worktree.list()` is a
  # DataFrame. Pure Python over the bundled sh/nix/polars; cross-platform.
  worktreePythonSource = builtins.path {
    name = "ix-mcp-worktree-python-source";
    path = ./src/worktree;
  };
  worktreeModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-worktree-python-module"
      {
        strictDeps = true;
        meta.description = "Git-worktree helper bundled into the ix-mcp interpreter";
      }
      ''
        site="$out/${pkgs.python3.sitePackages}/worktree"
        mkdir -p "$site"
        cp -r ${worktreePythonSource}/worktree/. "$site/"
      ''
  );
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

  # htpy: build HTML in plain Python (`div(class_="x")[ ... ]`), auto-escaping
  # every text node and attribute via markupsafe. Bundled so a session — and the
  # `view` renderer — can compose dashboard HTML without hand-rolling f-strings,
  # which is exactly where escaping is forgotten (the dtype-header XSS this
  # package set just had to patch). Not in nixpkgs; pure Python, one dep
  # (markupsafe). https://htpy.dev
  htpyModule =
    let
      pname = "htpy";
      version = "26.5.1";
    in
    pkgs.python3.pkgs.buildPythonPackage {
      inherit pname version;
      pyproject = true;
      src = pkgs.fetchPypi {
        inherit pname version;
        hash = "sha256-Q6NlwfxnAJTaeBuSOIMBkznOwDE5fWHV/l+OLyJ4tj4=";
      };
      # setuptools-scm reads the version from the sdist's PKG-INFO, but pin it so
      # the build never depends on a .git that the sdist does not carry.
      env.SETUPTOOLS_SCM_PRETEND_VERSION = version;
      build-system = [
        pkgs.python3.pkgs.setuptools
        pkgs.python3.pkgs.setuptools-scm
      ];
      # typing-extensions is only a dep below 3.13 (htpy's own marker); the
      # pinned interpreter is 3.13, so it is conditional rather than always-on.
      dependencies = [
        pkgs.python3.pkgs.markupsafe
      ]
      ++ lib.optional (lib.versionOlder pkgs.python3.pythonVersion "3.13") pkgs.python3.pkgs.typing-extensions;
      pythonImportsCheck = [ "htpy" ];
      doCheck = false;
    };

  # The interpreter the wrapper pins. Sessions build their venv from this with
  # `--system-site-packages`, so `tui`, `search`, `fff`, `exa_py`, numpy, polars
  # (incl. Postgres via psycopg + SQLAlchemy), duckdb, httpx, htpy, and playwright
  # are importable by default while an in-session `pip install` still writes to
  # the per-session venv.
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
      # htpy: compose HTML in Python with automatic escaping (see the module
      # definition above). The preferred way to build any dashboard markup.
      htpyModule
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
      # pygments: syntax highlighting for `view`'s Code views (cat/json/diff).
      ps.pygments
      # ansi2html: render a shell command's ANSI color to HTML for the `sh`
      # helper's human/dashboard view (the model view is escape-stripped).
      ps.ansi2html
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
      viewModule
      nixModule
      fleetModule
      shModule
      worktreeModule
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
  # The dashboard UI is a Svelte/Vite app under ./site, built by nix to one
  # self-contained index.html (viteSingleFile). The aiohttp dashboard server
  # (ix_notebook_mcp/dashboard.py) serves that file and feeds it the live
  # execution log over its REST API, so there is no committed build artifact and
  # no runtime asset dependency (the same shape as dashboard-core's embedded UI,
  # but read at runtime via IX_MCP_DASHBOARD_HTML since the server is Python).
  dashboardSiteSrc = lib.fileset.toSource {
    root = ./site;
    fileset = lib.fileset.intersection (lib.fileset.gitTracked ./.) ./site;
  };
  dashboardSite = ix.buildSvelteSite pkgs {
    pname = "ix-mcp-site";
    version = "0.1.0";
    src = dashboardSiteSrc;
    serve.enable = false;
    devServer = {
      name = "ix-mcp-site-dev";
      checkoutSubdir = "packages/mcp/site";
    };
  };
  dashboardHtml = "${dashboardSite}/share/ix-mcp-site/index.html";

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
          --set IX_MCP_VERSION ${lib.escapeShellArg ix.rev} \
          --set PLAYWRIGHT_BROWSERS_PATH ${lib.escapeShellArg playwrightBrowsers} \
          --set IX_GCAL_BIN ${lib.escapeShellArg "${gcalBin}/bin/gcal"} \
          --set IX_MCP_DASHBOARD_HTML ${lib.escapeShellArg dashboardHtml} \
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
  # htpy must import and auto-escape: a `<` in a text node comes out as `&lt;`.
  htpyBundled = importTest "htpy" "import htpy; print('htpy-ok' if '&lt;' in str(htpy.div['<']) else 'htpy-bad')";
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
            match = next((h for h in result.hits if "hello_world" in h.path), None)
            if match is not None:
                hit_path = match.path
                break
            time.sleep(0.25)
        assert hit_path is not None, f"fuzzy search did not find hello_world.txt: {result.hits!r}"

        grep_result = finder.grep("find me on this line", limit=10)
        files = {m.path for m in grep_result.matches}
        assert any("hello_world" in f for f in files), f"grep missed the txt file: {files!r}"
        assert any("main.rs" in f for f in files), f"grep missed main.rs: {files!r}"
        defs = finder.grep("fn main", mode="regex", classify_definitions=True)
        assert defs.matches, "regex grep returned no matches"

        glob_result = finder.glob("**/*.rs")
        assert any("main.rs" in h.path for h in glob_result.hits), (
            f"glob missed main.rs: {glob_result.hits!r}"
        )
    finally:
        finder.close()

    # CodeMap groups grep matches (defs first) and renders foldable per-file.
    cm = fff.CodeMap("find me on this line", grep_result.matches)
    assert cm.by_file and cm.matches and "find me on this line" in repr(cm), repr(cm)
    cm_html = cm._repr_html_()
    assert "<details" in cm_html and "find me" in cm_html, cm_html[:200]
    # map() is the grep -> CodeMap convenience (type only; avoids a scan race).
    assert isinstance(fff.map("fn main", root), fff.CodeMap)

    # The public search surface takes `path=` (consistent with `view`), not `root=`.
    import inspect as _inspect

    for _fn in (fff.find, fff.grep, fff.afind, fff.agrep, fff.map, fff.amap):
        _params = _inspect.signature(_fn).parameters
        assert "path" in _params and "root" not in _params, (_fn.__name__, list(_params))
    assert isinstance(fff.grep("find me on this line", path=root), fff.GrepResult)

    # A single file as `path` is grepped on its own (fff-c cannot content-index a
    # lone file as a root, so the helpers root at its parent and scope to it).
    main_rs = os.path.join(root, "src", "main.rs")
    one = fff.grep("find me on this line", path=main_rs)
    assert {m.path for m in one.matches} == {"main.rs"}, f"file grep not scoped: {one.matches!r}"
    assert fff.grep("no such content anywhere", path=main_rs).matches == [], "empty file grep should be empty"
    assert any(h.path == "main.rs" for h in fff.find("main", path=main_rs).hits), "file find missed it"
    assert fff.map("find me on this line", path=main_rs).by_file, "file map should group the hit"

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
    + "expected = {'python_exec','read','kernel_trace'}; "
    + "assert set(names) == expected, ('tool surface drifted: %r' % (names,)); "
    + "from ix_notebook_mcp import registry; instr = mcp._mcp_server.instructions; "
    + "assert 'root=' not in instr, 'a parameter/signature leaked into the instructions'; "
    + "assert '(query:' not in instr and '(path:' not in instr, 'a signature leaked into the instructions'; "
    + "missing = [m.name for m in registry.MODULES if ('`' + m.name + '`') not in instr]; "
    + "assert not missing, ('registry modules missing from instructions: %r' % (missing,)); "
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
        a = await run("import asyncio\nfor i in range(3):\n    print('A', i)\n    await asyncio.sleep(0.05)\nResult.text('A done')", budget=0.02, name="A")
        b = await run("import asyncio\nfor i in range(3):\n    print('B', i)\n    await asyncio.sleep(0.05)\nResult.text('B done')", budget=0.02, name="B")
        assert a.running() and b.running(), (a.status, b.status)
        assert len(jobs) == 2, len(jobs)
        await asyncio.sleep(0.5)
        assert a.status == "done" and b.status == "done", (a.status, b.status)
        assert "A 0" in a.output and "B 0" in b.output, (a.output, b.output)
        assert a.result.llm_result == "A done" and b.result.llm_result == "B done", (a.result, b.result)
        # paging ops over a finished job keep a large output recoverable
        assert "A 0" in a.head(10000) and a.slice(0, 1) == a.output[0]
        assert a.lines(0, 1).startswith("0: ")
        g = a.grep("A 1")
        assert "A 1" in g and g.split(":", 1)[0].strip().isdigit(), g
        assert "no lines match" in a.grep("nonesuch-xyz-pattern")
        # full sizes the server uses to detect a truncated reply
        s = runtime._job_summary(a)
        assert s["output_chars"] == len(a.output) and s["result_chars"] == len("A done"), s
        # history() indexes the runs and returns a Result naming both jobs
        h = ns["history"]()
        assert isinstance(h, runtime.Result) and a.id in h.llm_result and b.id in h.llm_result

        # A KeyboardInterrupt the user's own code raises keeps its real traceback
        # (it is NOT misattributed to the wedge watchdog, whose flag is unset here).
        k = await run("raise KeyboardInterrupt", budget=1.0, name="kbi")
        assert k.status == "error", k.status
        assert "Traceback" in k.error and "KeyboardInterrupt" in k.error, k.error
        assert "asyncio.to_thread" not in k.error, k.error

        # When the watchdog flag IS set (as the SIGUSR2 handler does before raising),
        # the same interrupt yields the actionable wedge message instead. The cell
        # sets the flag on its own running job via the runtime ContextVar.
        w = await run(
            "import ix_notebook_mcp.runtime as _rt\n"
            "_rt._ix_current.get().interrupted_by_watchdog = True\n"
            "raise KeyboardInterrupt",
            budget=1.0,
            name="kbi-watchdog",
        )
        assert w.status == "error", w.status
        assert "asyncio.to_thread" in w.error and "Traceback" not in w.error, w.error

        # A cell that prints but returns no Result fails the Result contract, and
        # the error now shows what it printed (stdout never reaches the model), so
        # a printing agent gets an actionable nudge instead of a silent dead end.
        p = await run("print('hello-from-stdout')", budget=1.0, name="printed")
        assert p.status == "error", p.status
        assert "printed to stdout" in p.error, p.error
        assert "hello-from-stdout" in p.error, p.error

        # A bare final value that already renders richly is auto-wrapped in
        # Result.of, so `df` on the last line just works without an explicit Result.
        d = await run("import polars as pl\npl.DataFrame({'x': [1, 2]})", budget=2.0, name="auto-df")
        assert d.status == "done", (d.status, d.error)
        assert isinstance(d.result, runtime.Result), type(d.result)
        # A plain scalar is not displayable, so the contract still rejects it.
        sc = await run("1 + 1", budget=2.0, name="scalar")
        assert sc.status == "error", sc.status

        # .result raises while the job runs (a misleading None would read as
        # "finished with no value"); .done()/.ok track the lifecycle.
        slow = await run("import asyncio\nawait asyncio.sleep(0.4)\nResult.text('late')", budget=0.02, name="slow")
        assert slow.running() and not slow.done(), slow.status
        try:
            _ = slow.result
            raise AssertionError("expected .result to raise while running")
        except runtime.JobStillRunning:
            pass
        await slow.task
        assert slow.done() and slow.ok, slow.status
        assert slow.result.llm_result == "late", slow.result

    asyncio.run(main())
    # api(): a discoverable catalog of kernel builtins + bundled modules.
    cat = ns["api"]()
    names = set(cat["name"].to_list())
    assert {"Result", "cells", "jobs", "sh", "api"} <= names, names
    filt = ns["api"]("cells")
    assert 1 <= filt.height <= cat.height, (filt.height, cat.height)

    # fff and view are pre-bound in the namespace (no import needed), the way
    # Result/cells/jobs/sh are, so fff.grep(...) / view.tree(...) just work.
    assert callable(getattr(ns.get("fff"), "grep", None)), ns.get("fff")
    assert callable(getattr(ns.get("view"), "tree", None)), ns.get("view")

    # Result.llm_images downscale a large raster to <= _IMAGE_MAX_DIM on its
    # longest edge before base64-encoding it for the model (Pillow is present via
    # matplotlib), so a full-page screenshot does not cost vision tokens at full
    # resolution.
    import base64 as _b64
    import io as _io

    from PIL import Image as _Image

    _buf = _io.BytesIO()
    _Image.new("RGB", (3000, 1500), (10, 20, 30)).save(_buf, format="PNG")
    _coerced = runtime._coerce_image(_buf.getvalue())
    assert _coerced is not None, _coerced
    _w, _h = _Image.open(_io.BytesIO(_b64.b64decode(_coerced["data"]))).size
    assert max(_w, _h) <= runtime._IMAGE_MAX_DIM, (_w, _h, runtime._IMAGE_MAX_DIM)

    # outputs.text() renders an over-cap block as a head+tail preview (not a
    # one-sided clip) with paging guidance, and honours IX_MCP_MAX_RESULT_CHARS.
    import importlib as _il
    import os as _os

    from ix_notebook_mcp import outputs as _outputs

    _os.environ["IX_MCP_MAX_RESULT_CHARS"] = "1000"
    _il.reload(_outputs)
    _blk = _outputs.text("HEAD" + ("z" * 5000) + "TAIL").text
    assert _blk.startswith("HEAD") and _blk.endswith("TAIL"), _blk[:40]
    assert "output too large" in _blk and len(_blk) < 2000, len(_blk)
    _os.environ.pop("IX_MCP_MAX_RESULT_CHARS", None)
    _il.reload(_outputs)

    # view.tree lists but does not descend into heavy dirs (node_modules, ...)
    # unless all=True, so a project's structure is not buried under vendored files.
    import pathlib as _pl
    import tempfile as _tf

    import view as _view

    _root = _tf.mkdtemp()
    _pl.Path(_root, "src").mkdir()
    _pkg = _pl.Path(_root, "node_modules", "pkg")
    _pkg.mkdir(parents=True)
    (_pkg / "index.js").write_text("x")
    _collapsed = _view.tree(_root, depth=3)
    _walked = _view.tree(_root, depth=3, all=True)
    assert _walked.height > _collapsed.height, (_collapsed.height, _walked.height)
    _names = _collapsed["name"].to_list()
    assert any("node_modules" in n for n in _names), _names
    assert not any("index.js" in n for n in _names), _names

    # .gitignore-aware pruning (git is on PATH in this sandbox): a dir the repo
    # ignores but that is NOT in the static denylist still collapses, and an
    # ignored file drops entirely.
    import shutil as _shutil
    import subprocess as _sub

    if _shutil.which("git"):
        _g = _tf.mkdtemp()
        _pl.Path(_g, "src").mkdir()
        _gen = _pl.Path(_g, "generated")
        _gen.mkdir()
        (_gen / "big.py").write_text("x")
        (_pl.Path(_g) / "debug.log").write_text("x")
        (_pl.Path(_g) / ".gitignore").write_text("generated/" + chr(10) + "*.log" + chr(10))
        _sub.run(["git", "init", "-q"], cwd=_g, check=True)
        _gi = _view.tree(_g, depth=3)["name"].to_list()
        assert any("generated" in n for n in _gi), _gi
        assert not any("big.py" in n for n in _gi), _gi
        assert not any("debug.log" in n for n in _gi), _gi
        assert any("src" in n for n in _gi), _gi

        # view.ls stays flat but flags git-ignored entries in an `ignored` column
        # (it never drops them, unlike tree): the *.log file is ignored, src is not.
        _lsg = _view.ls(_g)
        assert "ignored" in _lsg.columns, _lsg.columns
        _byname = {r["name"]: r["ignored"] for r in _lsg.iter_rows(named=True)}
        assert _byname.get("debug.log") is True, _byname
        assert _byname.get("src") is False, _byname

    print("runtime-ok")
  '';
  runtimeSmoke =
    pkgs.runCommand "ix-mcp-runtime-smoke"
      {
        # git is on PATH so the view.tree .gitignore-pruning assertion can init a
        # throwaway repo; without it that path falls back to the denylist (still
        # covered by the no-git case in the same test).
        nativeBuildInputs = [
          mcpPython
          pkgs.git
        ];
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

  # Boots a real kernel and proves the two signal-driven recoveries for a cell
  # that blocks the kernel's event loop with a synchronous call:
  #   1. kernel_trace (SIGUSR1 -> faulthandler) returns the kernel's stack WHILE
  #      the loop is wedged, since it never touches the execute channel.
  #   2. the wedge watchdog (SIGUSR2 -> KeyboardInterrupt) breaks the block past
  #      budget+grace, returns a 'wedged' summary in about budget+grace (not the
  #      sleep's full duration), and leaves the kernel usable for the next cell.
  # Guards the fix for the opaque "Timeout waiting for output" a forgotten
  # blocking call used to cause. SIGINT is NOT enough here: every cell is async
  # (await __ix_exec), and ipykernel interrupts async cells by cancelling the
  # asyncio task, which a synchronous call never yields to.
  wedgeTestPy = pkgs.writeText "ix-mcp-wedge-test.py" ''
    import asyncio
    import os
    import tempfile
    from pathlib import Path

    from ix_notebook_mcp import cli
    from ix_notebook_mcp.config import Config
    from ix_notebook_mcp.kernel import Kernel

    # Install the shipped IPython startup so the in-kernel runtime (__ix_exec,
    # Result, jobs, the SIGUSR1/SIGUSR2 handlers) loads in the booted kernel,
    # exactly as the CLI wires it.
    os.environ["IPYTHONDIR"] = str(cli._prepare_ipython_startup(0))
    config = Config(workdir=Path(tempfile.mkdtemp()), wedge_grace=1.0, max_budget=2.0)


    async def main():
        kernel = Kernel(config)
        await kernel.start()
        try:
            loop = asyncio.get_running_loop()

            # (1) A trace must come back even while a cell blocks the loop. Start a
            # blocking cell (budget high enough that the watchdog does not fire),
            # let it enter the sleep, then dump the kernel stack out of band.
            blocking = asyncio.ensure_future(
                kernel.python_exec("import time\ntime.sleep(6)\nResult.ok('slept')", budget=30.0, name="blk")
            )
            await asyncio.sleep(1.0)
            trace = await kernel.dump_trace()
            assert "Thread" in trace and 'File "' in trace, ("not a faulthandler dump", trace)
            _, blk = await blocking
            assert blk is not None and blk["status"] == "done", blk

            # (2) A cell that blocks past budget+grace is interrupted via SIGUSR2
            # and the kernel is usable for the next cell.
            started = loop.time()
            _, summary = await kernel.python_exec(
                "import time\ntime.sleep(30)\nResult.ok('done')", budget=0.5, name="block"
            )
            elapsed = loop.time() - started
            assert summary is not None and summary["status"] == "wedged", summary
            assert elapsed < 15, ("watchdog did not fire promptly", elapsed)
            assert "asyncio.to_thread" in summary["error"], summary

            _, after = await kernel.python_exec("Result.text('alive')", budget=10.0, name="after")
            assert after is not None and after["status"] == "done", after
            assert after["result"] is not None, after

            # (3) Cancelling an in-flight python_exec (the client cancels the call)
            # must not desync the shared shell channel. Start a cell that
            # backgrounds at its small budget, cancel the foreground wait while the
            # reply is in flight, then prove a later call still runs.
            inflight = asyncio.ensure_future(
                kernel.python_exec("await asyncio.sleep(5)\nResult.ok('slept')", budget=0.4, name="cancelme")
            )
            await asyncio.sleep(0.1)
            inflight.cancel()
            try:
                await inflight
            except asyncio.CancelledError:
                pass
            _, revived = await kernel.python_exec("Result.text('post-cancel')", budget=10.0, name="post-cancel")
            assert revived is not None and revived["status"] == "done", revived
            assert revived["result"] is not None, revived

            # (4) The python_exec TOOL clamps an oversized budget to max_budget so a
            # giant foreground wait cannot sit on the one shell channel: the call
            # returns within the cap (not the requested 600s) and says it clamped.
            from ix_notebook_mcp import tools
            from ix_notebook_mcp.config import set_config
            from ix_notebook_mcp.kernel import set_kernel

            set_config(config)
            set_kernel(kernel)
            started = loop.time()
            clamped = await tools.python_exec(
                "await asyncio.sleep(30)\nResult.ok('done')", budget=600.0, name="bigbudget"
            )
            elapsed = loop.time() - started
            assert elapsed < 10, ("budget was not clamped", elapsed)
            note = " ".join(getattr(c, "text", "") or "" for c in clamped)
            assert "clamped" in note, note
        finally:
            await kernel.shutdown()


    asyncio.run(main())
    print("wedge-ok")
  '';
  wedgeSmoke =
    pkgs.runCommand "ix-mcp-wedge-smoke"
      {
        nativeBuildInputs = [ mcpPython ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"
        ${lib.getExe mcpPython} ${wedgeTestPy} >stdout 2>stderr || {
          echo "ix-mcp wedge smoke failed:" >&2
          cat stdout stderr >&2
          exit 1
        }
        grep -qx 'wedge-ok' stdout || {
          echo "ix-mcp wedge smoke did not confirm the watchdog:" >&2
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

    # A tuple/list carrying a rich value (a DataFrame) renders each element with
    # its own view, stacked, instead of stringifying the frame into a one-column
    # table -- Result((repr_text, df)) shows the text AND the real table.
    stacked = runtime.Result.of(("GrepResult: 0 matches", pl.DataFrame({"a": [1, 2]})))
    assert stacked.user_html.count("<table") == 1, stacked.user_html[:200]
    assert "GrepResult: 0 matches" in stacked.llm_result and "shape:" in stacked.llm_result, stacked.llm_result
    # A plain list of scalars is still ONE table (not stacked), unchanged.
    scalars = runtime.Result.of([1, 2, 3])
    assert scalars.user_html.count("<table") == 1, scalars.user_html[:200]
    # Stacking preserves a nested Result's model images (Result.of copies a
    # Result faithfully instead of rebuilding it from its display bundle).
    inner = runtime.Result(user_html="<b>x</b>", llm_result="x", llm_images=[b"\x89PNG\r\n"])
    nested = runtime.Result.of([inner, pl.DataFrame({"a": [1]})])
    assert len(nested.llm_images) == 1, ("nested Result dropped its images", nested.llm_images)


    async def main():
        # A DataFrame result is stored with its text/html bundle.
        df_job = await run("Result.of(pl.DataFrame({'a': [1, 2], 'b': ['x', 'y']}))", budget=3.0, name="df")
        await df_job.task
        conn = sqlite3.connect(store_path)
        conn.row_factory = sqlite3.Row
        row = conn.execute("SELECT status, outputs FROM executions WHERE id = ?", (df_job.id,)).fetchone()
        assert row["status"] == "done", row["status"]
        result_mimes = {mime for out in json.loads(row["outputs"]) for mime in out["data"]}
        assert "text/html" in result_mimes, ("result mimes", result_mimes)

        # An htpy element renders through the __html__ protocol: IPython's html
        # formatter ignores __html__ by default, so without _register_rich_formatters
        # cells.add/Result.of would store the element's repr instead of its HTML.
        htpy_job = await run(
            "import htpy\nResult.of(htpy.div(class_='x')['<hi>'])", budget=3.0, name="htpy"
        )
        await htpy_job.task
        htpy_outputs = conn.execute(
            "SELECT outputs FROM executions WHERE id = ?", (htpy_job.id,)
        ).fetchone()[0]
        htpy_html = [out["data"].get("text/html") for out in json.loads(htpy_outputs)][-1]
        assert htpy_html == '<div class="x">&lt;hi&gt;</div>', htpy_html

        # A display() call made while a job runs is captured too.
        disp_job = await run(
            "from IPython.display import display\ndisplay(pl.DataFrame({'z': [9]}))\nResult.ok('shown')",
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

        # Result DWIM: a bare value renders like Result.of (no user_html boilerplate).
        # A dict becomes a table -- a valid text/html string, not a raw dict that
        # breaks nbformat -- and its keys reach the model text.
        dwim_job = await run("Result({'alpha': 1, 'beta': 2})", budget=3.0, name="dwim")
        await dwim_job.task
        dwim_row = conn.execute(
            "SELECT status, outputs FROM executions WHERE id = ?", (dwim_job.id,)
        ).fetchone()
        assert dwim_row["status"] == "done", dwim_row["status"]
        dwim_bundle = [out["data"] for out in json.loads(dwim_row["outputs"])][-1]
        assert isinstance(dwim_bundle.get("text/html"), str) and dwim_bundle["text/html"], dwim_bundle
        assert "alpha" in dwim_bundle.get("text/plain", "") and "beta" in dwim_bundle["text/plain"], dwim_bundle

        # Multiple values are ALL shown (not silently collapsed to the first).
        multi_job = await run("Result(True, [1, 2, 3])", budget=3.0, name="multi")
        await multi_job.task
        multi_row = conn.execute(
            "SELECT status, outputs FROM executions WHERE id = ?", (multi_job.id,)
        ).fetchone()
        assert multi_row["status"] == "done", multi_row["status"]
        multi_text = [out["data"].get("text/plain", "") for out in json.loads(multi_row["outputs"])][-1]
        # Both values are shown: the bool by its repr, the list as its one-column
        # frame (CSV rows 1/2/3), not collapsed to just the first value.
        assert "True" in multi_text and "1\n2\n3" in multi_text, ("multi-value dropped a value", multi_text)


    asyncio.run(main())
    print("rich-ok")
  '';
  # Proves the yielding-cell contract end to end: a cell that `yield Result(...)`
  # streams every yielded Result to the store (the dashboard) and to the model
  # (to_mcp), keeps its top-level names in the namespace like a normal cell, and a
  # non-Result yield fails the run. A plain (non-yielding) cell is unchanged. In
  # process (a shell, the store), no kernel boot or network, so the sandbox runs it.
  yieldTestPy = pkgs.writeText "ix-mcp-yield-test.py" ''
    import asyncio
    import json
    import os
    import sqlite3
    import tempfile

    from IPython.core.interactiveshell import InteractiveShell

    InteractiveShell.instance()

    store_path = tempfile.mktemp(suffix=".db")
    os.environ["IX_MCP_STORE"] = store_path

    from ix_notebook_mcp import outputs, runtime

    ns = {}
    runtime.install(ns)
    run = ns["__ix_run"]


    async def main():
        conn = sqlite3.connect(store_path)
        conn.row_factory = sqlite3.Row

        # A yielding cell streams multiple Results; its top-level names persist.
        code = (
            "acc = 0\n"
            "for i in range(3):\n"
            "    acc += i\n"
            "    yield Result.ok(f'step {i}')\n"
            "yield Result.of(acc)"
        )
        job = await run(code, budget=3.0, name="yield")
        await job.task
        assert job.status == "done", (job.status, job.error)
        assert ns["acc"] == 3, ns.get("acc")
        outs = json.loads(
            conn.execute("SELECT outputs FROM executions WHERE id = ?", (job.id,)).fetchone()[0]
        )
        htmls = [o["data"].get("text/html") for o in outs if "text/html" in o["data"]]
        assert len(htmls) == 4, ("expected 4 yielded results", len(htmls), outs)

        # Each yielded Result reaches the model: to_mcp over the stored bundles
        # hands back the llm text for every one.
        mcp = outputs.to_mcp(
            [{"output_type": "display_data", "data": o["data"], "metadata": {}} for o in outs]
        )
        texts = [c.text for c in mcp if getattr(c, "text", None) is not None]
        assert "step 0" in texts and "3" in texts, texts

        # A non-Result yield fails the run with the contract message.
        bad = await run("yield 123", budget=3.0, name="bad")
        await bad.task
        assert bad.status == "error", bad.status
        assert "was not a Result" in (bad.error or ""), bad.error

        # A normal (non-yielding) cell is unchanged: it must still end with a Result.
        plain = await run("Result.ok('plain')", budget=3.0, name="plain")
        await plain.task
        assert plain.status == "done", (plain.status, plain.error)


    asyncio.run(main())
    print("yield-ok")
  '';

  yieldSmoke =
    pkgs.runCommand "ix-mcp-yield-smoke"
      {
        nativeBuildInputs = [ mcpPython ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"
        ${lib.getExe mcpPython} ${yieldTestPy} >stdout 2>stderr || {
          echo "ix-mcp yield smoke failed:" >&2
          cat stdout stderr >&2
          exit 1
        }
        grep -qx 'yield-ok' stdout || {
          echo "ix-mcp yield smoke did not confirm yielded results:" >&2
          cat stdout stderr >&2
          exit 1
        }
        mkdir -p "$out"
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

  # Exercises the live-value introspection that feeds the dashboard's hover/inlay:
  # describe() classifies scalars, DataFrames, functions (with a source location),
  # and modules; cell_bindings() resolves a cell's mentioned names against the
  # namespace (excluding attribute parts); and a finished job persists those
  # bindings to the store, which is where the dashboard reads them. In-process, no
  # kernel or network, so the sandbox runs it.
  bindingsTestPy = pkgs.writeText "ix-mcp-bindings-test.py" ''
    import asyncio
    import inspect
    import json
    import os
    import sqlite3
    import tempfile

    import polars as pl

    from ix_notebook_mcp import introspect

    # Direct descriptors: each kind carries the inlay summary the dashboard shows.
    assert introspect.describe(42)["summary"] == "42"
    df_desc = introspect.describe(pl.DataFrame({"a": [1, 2, 3], "b": ["x", "y", "z"]}))
    assert df_desc["kind"] == "dataframe" and "3×2" in df_desc["summary"], df_desc

    # A wide frame's schema detail is capped, not dumped whole, so the stored row
    # and poll payload stay bounded.
    wide = introspect.describe(pl.DataFrame({f"c{i}": [0] for i in range(30)}))
    assert "+6 more" in wide["detail"], wide

    def sample(x):
        "a doc line"
        return x

    fn_desc = introspect.describe(sample)
    assert fn_desc["kind"] == "callable" and fn_desc["summary"].startswith("ƒ sample"), fn_desc
    # A function has a definition site: this is the go-to-definition payload.
    assert ":" in fn_desc.get("def", ""), fn_desc

    mod_desc = introspect.describe(inspect)
    assert mod_desc["kind"] == "module" and mod_desc["summary"] == "module inspect", mod_desc

    # cell_bindings resolves names a cell mentions; an attribute (df.height) is not
    # a name, so only `df` and `n` are described, not `height`.
    ns = {"df": pl.DataFrame({"a": [1]}), "n": 7}
    bound = introspect.cell_bindings("rows = df.height\ntotal = n + 1\n", ns)
    assert set(bound) == {"df", "n"}, bound
    assert bound["df"]["kind"] == "dataframe" and bound["n"]["summary"] == "7", bound

    # The highlighter marks each identifier token with data-ix-name, the anchor the
    # browser joins with bindings; attribute parts (head) are not names so the
    # frontend never lights them up, but the token is still present in the markup.
    from ix_notebook_mcp import dashboard

    highlighted = dashboard._code_html("rows = df.head()\ntotal = n + 1\n")
    assert 'data-ix-name="df"' in highlighted, highlighted
    assert 'data-ix-name="rows"' in highlighted, highlighted
    assert 'data-ix-name="total"' in highlighted, highlighted

    # Opening a pre-bindings store migrates it, and a second open (the kernel and
    # dashboard each open the store) is a no-op rather than an error.
    from ix_notebook_mcp import store as store_mod

    legacy = tempfile.mktemp(suffix=".db")
    seed = sqlite3.connect(legacy)
    seed.execute(
        "CREATE TABLE executions (id TEXT PRIMARY KEY, name TEXT, code TEXT NOT NULL, "
        "status TEXT NOT NULL, started_at REAL NOT NULL, ended_at REAL, "
        "output TEXT, result TEXT, error TEXT, outputs TEXT)"
    )
    seed.commit()
    seed.close()
    conn_a = store_mod.connect(legacy)
    store_mod.connect(legacy)
    migrated = {row[1] for row in conn_a.execute("PRAGMA table_info(executions)")}
    assert "bindings" in migrated, migrated

    # The duplicate-column race itself: a connection that observed the column
    # missing (here forced via a shim) but runs ALTER after another connection
    # already added it must swallow the error, not raise. This exercises the
    # except branch the idempotency case above skips.
    class _StaleSchema:
        def __init__(self, conn):
            self._conn = conn

        def execute(self, sql, *args):
            if sql.startswith("PRAGMA table_info"):
                return [(0, "id"), (1, "name")]  # pretend bindings is still absent
            return self._conn.execute(sql, *args)

    store_mod._migrate(_StaleSchema(conn_a))  # ALTER -> duplicate column -> caught

    # End to end: a finished job snapshots its bindings into the store row.
    store_path = tempfile.mktemp(suffix=".db")
    os.environ["IX_MCP_STORE"] = store_path

    from IPython.core.interactiveshell import InteractiveShell

    InteractiveShell.instance()

    from ix_notebook_mcp import runtime

    user_ns = {"pl": pl}
    runtime.install(user_ns)
    run = user_ns["__ix_run"]


    async def main():
        job = await run("frame = pl.DataFrame({'a': [1, 2]})\nResult.ok('made it')", budget=3.0, name="bind")
        await job.task
        conn = sqlite3.connect(store_path)
        conn.row_factory = sqlite3.Row
        row = conn.execute("SELECT bindings FROM executions WHERE id = ?", (job.id,)).fetchone()
        stored = json.loads(row["bindings"])
        assert stored.get("frame", {}).get("kind") == "dataframe", stored
        # `pl` is referenced and live, so it is described as a module.
        assert stored.get("pl", {}).get("kind") == "module", stored


    asyncio.run(main())
    print("bindings-ok")
  '';
  bindingsSmoke =
    pkgs.runCommand "ix-mcp-bindings-smoke"
      {
        nativeBuildInputs = [ mcpPython ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"
        ${lib.getExe mcpPython} ${bindingsTestPy} >stdout 2>stderr || {
          echo "ix-mcp bindings smoke failed:" >&2
          cat stdout stderr >&2
          exit 1
        }
        grep -qx 'bindings-ok' stdout || {
          echo "ix-mcp bindings smoke did not confirm value introspection:" >&2
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

  # The view module: tabular helpers return plain polars DataFrames (so they stay
  # composable), the file helpers return a Code view whose repr is the raw text,
  # and df_html renders the styled table the kernel installs globally. Pure local
  # FS over the bundled fff/polars/pygments, so the sandbox runs it.
  viewTestPy = pkgs.writeText "ix-mcp-view-test.py" ''
    import polars as pl

    import view

    base = "${./.}"

    lsdf = view.ls(base)
    assert isinstance(lsdf, pl.DataFrame) and "kind" in lsdf.columns, lsdf.columns
    # ls flags git-ignored entries in an `ignored` Boolean column rather than
    # dropping them; outside a git work tree (this store path) nothing is ignored.
    assert lsdf.schema["ignored"] == pl.Boolean, lsdf.schema
    assert not lsdf["ignored"].any(), lsdf
    # A DataFrame stays a DataFrame through polars ops (composable).
    assert isinstance(lsdf.filter(pl.col("kind") == "dir"), pl.DataFrame)

    g = view.grep("viewTestPy", base)
    assert isinstance(g, pl.DataFrame) and set(g.columns) == {"path", "line", "text"}, g.columns
    assert g.height > 0, "expected a grep hit for the marker"

    f = view.find("default.nix", base)
    assert isinstance(f, pl.DataFrame) and "path" in f.columns

    tr = view.tree(base, depth=1)
    assert isinstance(tr, pl.DataFrame) and "depth" in tr.columns

    out = view.df_html(lsdf)
    assert "<table" in out and "rows" in out and "tabular-nums" in out, out[:120]

    # Nested List(Struct)/Struct cells render as boxed sub-tables, not a
    # truncated str(value): the inner field values must reach the HTML.
    nested = pl.DataFrame({"host": ["h1"]}).with_columns(
        mounts=pl.lit([{"mount": "/data", "pct": 91}], dtype=pl.List(pl.Struct({"mount": pl.String, "pct": pl.Int64})))
    )
    nout = view.df_html(nested)
    assert "/data" in nout and ">91<" in nout, nout[:200]
    # A nested cell is a real sub-table (outer + inner), not a truncated repr.
    assert nout.count("<table") >= 2 and "[{" not in nout, nout[:200]

    # A struct field name is attacker-controllable (any frame built from
    # untrusted data); it must be HTML-escaped both in the column-header dtype
    # string and in the nested sub-table, never injected as live markup.
    evil = pl.DataFrame({"x": [1]}).with_columns(
        rec=pl.lit({"<img src=x>": 1}, dtype=pl.Struct({"<img src=x>": pl.Int64}))
    )
    eout = view.df_html(evil)
    # The `not in` clause is the S1 regression guard: it fails on the unfixed
    # header that interpolated the dtype string raw.
    assert "<img src=x>" not in eout and "&lt;img src=x&gt;" in eout, eout[:300]

    c = view.cat(base + "/default.nix", lines=(1, 3))
    assert isinstance(c, view.Code)
    assert repr(c).count("\n") <= 3
    assert "span" in c._repr_html_().lower()

    j = view.json({"a": [1, 2], "b": None})
    assert '"a"' in repr(j) and "span" in j._repr_html_().lower()

    d = view.diff("x\ny\n", "x\nz\n")
    assert "-y" in repr(d) and "+z" in repr(d)

    # edit() applies a replacement and returns it as a highlighted diff.
    import pathlib as _pl_path
    import tempfile as _tmp
    ep = _pl_path.Path(_tmp.mkdtemp()) / "f.txt"
    ep.write_text("alpha\nbeta\n")
    ed = view.edit(ep, "beta", "gamma")
    assert isinstance(ed, view.Code) and "-beta" in repr(ed) and "+gamma" in repr(ed), repr(ed)
    assert ep.read_text() == "alpha\ngamma\n", ep.read_text()
    try:
        view.edit(ep, "missing-zzz", "q")
    except ValueError:
        pass
    else:
        raise SystemExit("edit should raise on a missing pattern")
    prev = view.edit(ep, "gamma", "delta", dry_run=True)
    assert "+delta" in repr(prev) and ep.read_text() == "alpha\ngamma\n", "dry_run must not write"

    print("view-ok")
  '';
  viewSmoke =
    pkgs.runCommand "ix-mcp-view-smoke"
      {
        nativeBuildInputs = [ mcpPython ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"
        ${lib.getExe mcpPython} ${viewTestPy} >stdout 2>stderr || {
          echo "ix-mcp view smoke failed:" >&2
          cat stdout stderr >&2
          exit 1
        }
        grep -qx 'view-ok' stdout || {
          echo "ix-mcp view smoke did not confirm the view module:" >&2
          cat stdout stderr >&2
          exit 1
        }
        mkdir -p "$out"
      '';

  # The sh module: runs a real subprocess on the loop and proves the human/model
  # split. The command emits ANSI color; the dashboard view (_repr_html_ /
  # user_html) must carry that color as HTML while the model view (repr /
  # llm_result) is escape-stripped. Also guards the Result contract (an Output is
  # a Result, so a cell can end with it), exit-code capture, and check=True.
  # Pure local subprocess over the bundled ansi2html, so the sandbox runs it.
  shTestPy = pkgs.writeText "ix-mcp-sh-test.py" ''
    import asyncio

    import sh
    from ix_notebook_mcp.runtime import Result


    async def main():
        # A command that emits an SGR color escape around its output.
        colored = await sh.sh(r"printf '\033[31mred\033[0m\n'", cwd=".")
        assert colored.ok and colored.code == 0, colored.code
        # Model view: no escape bytes, the word survives.
        assert "\x1b" not in colored.text and "red" in colored.text, repr(colored.text)
        assert "\x1b" not in colored.llm_result, repr(colored.llm_result)
        # Human view: color rendered to HTML (a styled span), no raw escapes.
        html = colored._repr_html_()
        assert "\x1b" not in html and "span" in html.lower(), html[:200]
        assert "color" in html.lower(), html[:200]
        # An Output IS a Result, so ending a cell with it satisfies the contract.
        assert isinstance(colored, Result), type(colored)

        # argv form, and a non-zero exit is surfaced (not swallowed).
        failed = await sh.sh(["false"], cwd=".")
        assert not failed.ok and failed.code == 1, failed.code
        assert "[exit 1]" in failed.llm_result, failed.llm_result

        # check=True turns a non-zero exit into a typed error carrying the output.
        try:
            await sh.sh("exit 3", check=True, cwd=".")
        except sh.ShellError as exc:
            assert exc.output.code == 3, exc.output.code
        else:
            raise SystemExit("expected ShellError on a non-zero exit with check=True")

        # An OSC-8 hyperlink (what gh/eza emit under FORCE_COLOR) is a non-CSI
        # escape: the stripper must remove its \x1b bytes too, not just SGR color.
        osc = await sh.sh(r"printf '\033]8;;https://x\033\\link\033]8;;\033\\\n'", cwd=".")
        assert "\x1b" not in osc.text and "link" in osc.text, repr(osc.text)
        assert "\x1b" not in osc.llm_result, repr(osc.llm_result)

        # A timeout must terminate the command's whole group and return promptly,
        # even when the command backgrounds a child that holds the stdout pipe
        # (the case where a naive kill + reap hangs forever).
        loop = asyncio.get_running_loop()
        start = loop.time()
        try:
            await sh.sh("sleep 30 & echo started; wait", timeout=0.5, cwd=".")
        except TimeoutError:
            pass
        else:
            raise SystemExit("expected TimeoutError from a command that outlives its timeout")
        elapsed = loop.time() - start
        assert elapsed < 10, f"timeout did not return promptly: {elapsed:.1f}s"

        # The module object itself is callable: the documented
        # `import sh; await sh(cmd)` works without reaching for `sh.sh`.
        assert callable(sh), "sh module is not callable"
        direct = await sh("printf hi", cwd=".")
        assert direct.ok and direct.text == "hi", repr(direct.text)

        print("sh-ok", sh.__version__)


    asyncio.run(main())
  '';
  shSmoke =
    pkgs.runCommand "ix-mcp-sh-smoke"
      {
        nativeBuildInputs = [ mcpPython ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"
        ${lib.getExe mcpPython} ${shTestPy} >stdout 2>stderr || {
          echo "ix-mcp sh smoke failed:" >&2
          cat stdout stderr >&2
          exit 1
        }
        grep -q '^sh-ok' stdout || {
          echo "ix-mcp sh smoke did not confirm the sh module:" >&2
          cat stdout stderr >&2
          exit 1
        }
        mkdir -p "$out"
      '';

  # The fleet module: a mocked asyncssh connection drives the fan-out so the test
  # never depends on a reachable host or a running sshd. It asserts the contract
  # that matters: import works, the semaphore caps in-flight connections,
  # tag_host adds the column, diagonal_relaxed merges mismatched schemas, and
  # on_error="collect" survives a failing host (recording it in attrs). Pure
  # Python over the bundled asyncssh + polars, so the sandbox runs it.
  fleetTestPy = pkgs.writeText "ix-mcp-fleet-test.py" ''
    import asyncio
    import sys

    import polars as pl

    import fleet

    # --- Mock asyncssh so no network/sshd is needed. ---------------------------
    inflight = {"now": 0, "max": 0}

    class _Result:
        def __init__(self, stdout):
            self.stdout = stdout

    class _Conn:
        def __init__(self, host):
            self.host = host

        async def __aenter__(self):
            inflight["now"] += 1
            inflight["max"] = max(inflight["max"], inflight["now"])
            await asyncio.sleep(0.02)  # hold the slot so overlap is observable
            return self

        async def __aexit__(self, *exc):
            inflight["now"] -= 1
            return False

        async def run(self, command, encoding=None, check=True):
            # One host is configured to fail, to exercise on_error="collect".
            if self.host == "bad":
                raise RuntimeError("boom")
            # Two hosts return DIFFERENT schemas to exercise diagonal_relaxed:
            # one emits {a}, the other {b}.
            if self.host == "h1":
                return _Result(b'{"a": 1}\n')
            return _Result(b'{"b": 2}\n')

    def _connect(**opts):
        return _Conn(opts["host"])

    fleet.asyncssh.connect = _connect

    async def main():
        hosts = ["h1", "h2", "bad"]
        df = await fleet.scan(hosts, "noop", concurrency=2, on_error="collect")

        # tag_host added the host column, first.
        assert df.columns[0] == "host", df.columns
        # diagonal_relaxed unioned the {a} and {b} schemas.
        assert set(df.columns) == {"host", "a", "b"}, df.columns
        # Two good hosts -> two rows; the bad host did not abort the batch.
        assert df.height == 2, df.height
        # The failing host is recorded, not raised.
        fails = df.attrs["fleet_failures"]
        assert list(fails) == ["bad:22"], fails
        assert "boom" in fails["bad:22"], fails

        # Semaphore actually capped concurrency at 2 even with 3 hosts.
        assert inflight["max"] <= 2, inflight

        # on_error="raise" aggregates failures into FleetError.
        try:
            await fleet.scan(["bad"], "noop", on_error="raise")
        except fleet.FleetError as exc:
            assert "bad:22" in exc.failures, exc.failures
        else:
            raise AssertionError("expected FleetError")

        # read_text yields one row per line with host+line columns.
        class _Text(_Conn):
            async def run(self, command, encoding=None, check=True):
                return _Result(b"line one\nline two\n")
        fleet.asyncssh.connect = lambda **o: _Text(o["host"])
        txt = await fleet.read_text(["h1"], "/var/log/x")
        assert set(txt.columns) == {"host", "line"}, txt.columns
        assert txt.height == 2, txt.height

        # An all-empty fan-out returns an empty frame, never crashes.
        class _Empty(_Conn):
            async def run(self, command, encoding=None, check=True):
                return _Result(b"")
        fleet.asyncssh.connect = lambda **o: _Empty(o["host"])
        empty = await fleet.scan(["x"], "noop")
        assert isinstance(empty, pl.DataFrame) and empty.height == 0

        print("fleet-ok", fleet.__version__)

    asyncio.run(main())
  '';
  fleetSmoke =
    pkgs.runCommand "ix-mcp-fleet-smoke"
      {
        nativeBuildInputs = [ mcpPython ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"
        ${lib.getExe mcpPython} ${fleetTestPy} >stdout 2>stderr || {
          echo "ix-mcp fleet smoke failed:" >&2
          cat stdout stderr >&2
          exit 1
        }
        grep -q '^fleet-ok' stdout || {
          echo "ix-mcp fleet smoke did not confirm the fleet module:" >&2
          cat stdout stderr >&2
          exit 1
        }
        mkdir -p "$out"
      '';

  # macOS-only modules (`screen`, `vmkit`) are only bundled on Darwin; their
  # import tests only exist there.
  # The `nix` module: parse a captured internal-json stream (no subprocess, no
  # network, so the sandbox runs it) and assert the durable event log, the folded
  # build-DAG view, the live error capture, and the rendered tree. The fixture is
  # a real-shaped slice of a `nix build` stream: an eval activity, a queryPathInfo
  # with a child fileTransfer carrying progress, a build with a log line and a set
  # phase, the matching stops, and a trailing error msg.
  nixTestPy = pkgs.writeText "ix-mcp-nix-test.py" ''
    import polars as pl

    import nix

    stream = "\n".join(
        [
            '@nix {"action":"start","id":1,"level":4,"parent":0,"text":"evaluating","type":0}',
            '@nix {"action":"start","id":2,"level":4,"parent":0,"text":"querying","type":109,"fields":["/nix/store/x","https://cache"]}',
            '@nix {"action":"start","id":3,"level":4,"parent":2,"text":"downloading","type":101,"fields":["https://cache/x.narinfo"]}',
            '@nix {"action":"result","id":3,"type":105,"fields":[2,3,0,0]}',
            '@nix {"action":"stop","id":3}',
            '@nix {"action":"stop","id":2}',
            '@nix {"action":"start","id":4,"level":3,"parent":0,"text":"building","type":105,"fields":["/nix/store/x.drv","",1,1]}',
            '@nix {"action":"result","id":4,"type":104,"fields":["unpackPhase"]}',
            '@nix {"action":"result","id":4,"type":101,"fields":["compiling thing"]}',
            "not an @nix line, ignored",
            '@nix {"action":"stop","id":4}',
            # Real nix escapes ANSI as \u001b (valid JSON); the parser strips it.
            '@nix {"action":"msg","level":0,"msg":"error: \u001b[31mboom\u001b[0m","raw_msg":"error: boom"}',
        ]
    )

    log = nix.parse(stream)

    # Durable event log: one row per @nix line (the plain line is skipped), typed.
    ev = log.events
    assert isinstance(ev, pl.DataFrame), type(ev)
    assert ev.height == 11, ev.height
    assert ev.filter(pl.col("action") == "start").height == 4, ev
    # kind decodes both activity and result enums; ANSI is stripped from msg.
    assert set(ev["kind"].drop_nulls().to_list()) >= {"build", "fileTransfer", "progress"}, ev["kind"]

    # Folded DAG view: one row per activity, with the parent edge and depth.
    acts = log.activities
    assert acts.height == 4, acts
    ft = acts.filter(pl.col("kind") == "fileTransfer").row(0, named=True)
    assert ft["parent"] == 2 and ft["depth"] == 1, ft
    assert ft["done"] == 2 and ft["expected"] == 3, ft  # progress folded in

    bld = acts.filter(pl.col("kind") == "build").row(0, named=True)
    assert bld["status"] == "done", bld          # stop folded in
    assert bld["phase"] == "unpackPhase", bld    # setPhase folded in
    assert bld["last_log"] == "compiling thing", bld  # build log line folded in
    assert bld["drv"] == "/nix/store/x.drv", bld

    # Error captured from the msg line, ANSI stripped.
    assert log.error == "error: boom", repr(log.error)

    # Tree + html render don't crash and reflect the build.
    assert "building" in log.tree()
    assert "<pre" in log.resource_html()

    # Empty parse yields well-typed empty frames, never crashes.
    empty = nix.parse("")
    assert empty.events.height == 0 and empty.activities.height == 0
    assert empty.events.schema["seq"] == pl.Int64

    # A warning (level 1) or an info line containing "error:" is NOT the failure.
    warn = nix.parse(
        '@nix {"action":"msg","level":1,"msg":"warning: deprecated"}\n'
        '@nix {"action":"msg","level":3,"msg":"note: see error: above"}'
    )
    assert warn.error is None, warn.error

    # Malformed parent cycle must not infinite-recurse the activities/render path.
    cyc = nix.parse(
        '@nix {"action":"start","id":1,"parent":2,"text":"a","type":0}\n'
        '@nix {"action":"start","id":2,"parent":1,"text":"b","type":0}'
    )
    assert cyc.activities.height == 2, cyc.activities
    assert "a" in cyc.tree()

    # Non-int progress fields must not crash the Int64 activities schema.
    bad = nix.parse(
        '@nix {"action":"start","id":1,"parent":0,"text":"x","type":101}\n'
        '@nix {"action":"result","id":1,"type":105,"fields":["nan","oops",0,0]}'
    )
    assert bad.activities.row(0, named=True)["done"] == 0, bad.activities

    # attrs(): the flake-show parser flattens systemed + plain outputs, filtered
    # to the requested system; an omitted (empty) system contributes no rows.
    show = {
        "packages": {
            "aarch64-darwin": {"mcp": {"type": "derivation", "description": "the mcp"}},
            "x86_64-linux": {},
        },
        "nixosConfigurations": {"host": {"type": "nixos-configuration"}},
    }
    rows = nix._flake_show_rows(show, "aarch64-darwin")
    by = {(r["kind"], r["attr"]): r for r in rows}
    assert by[("packages", "mcp")]["description"] == "the mcp", rows
    assert ("nixosConfigurations", "host") in by, rows
    assert all(r["attr"] for r in rows), rows
    assert isinstance(nix._current_system(), str) and "-" in nix._current_system()

    # eval(): the arg builder quotes nothing through a shell and substitutes the
    # {system} template, so `--apply` rides as its own argv element.
    assert nix._eval_args(".#mcp") == ["eval", ".#mcp", "--json", "--no-warn-dirty"]
    assert nix._eval_args(".#mcp", apply="builtins.attrNames") == [
        "eval", ".#mcp", "--json", "--no-warn-dirty", "--apply", "builtins.attrNames",
    ]
    assert nix._eval_args(".#x", raw=True)[2] == "--raw", nix._eval_args(".#x", raw=True)
    sysd = nix._current_system()
    assert nix._eval_args(".#checks.{system}.lint")[1] == f".#checks.{sysd}.lint"
    assert nix._eval_args(".#checks.{system}", system="x86_64-linux")[1] == ".#checks.x86_64-linux"

    print("nix-ok", nix.__version__)
  '';
  nixSmoke =
    pkgs.runCommand "ix-mcp-nix-smoke"
      {
        nativeBuildInputs = [ mcpPython ];
        strictDeps = true;
      }
      ''
        ${lib.getExe mcpPython} ${nixTestPy} >stdout 2>stderr || {
          echo "ix-mcp nix smoke failed:" >&2
          cat stdout stderr >&2
          exit 1
        }
        grep -q '^nix-ok' stdout || {
          echo "ix-mcp nix smoke did not confirm the nix module:" >&2
          cat stdout stderr >&2
          exit 1
        }
        mkdir -p "$out"
      '';

  # The worktree module: drive real `git worktree` against a throwaway repo
  # (git is on PATH in this sandbox). Proves add() creates a new branch in its
  # own tree (and checks out an existing branch instead of recreating it),
  # list() is a DataFrame marking the current tree, the Worktree is os.PathLike
  # and `wt / "x"` joins onto it, commit() stages new files, and remove() drops
  # the tree. Pure git + the bundled sh, so the sandbox runs it.
  worktreeTestPy = pkgs.writeText "ix-mcp-worktree-test.py" ''
    import asyncio
    import os
    import pathlib
    import subprocess
    import tempfile

    import polars as pl

    import worktree


    def _git(*args, cwd):
        subprocess.run(["git", "-C", cwd, *args], check=True, capture_output=True)


    async def main():
        repo = tempfile.mkdtemp()
        _git("init", "-q", cwd=repo)
        _git("config", "user.email", "t@t", cwd=repo)
        _git("config", "user.name", "t", cwd=repo)
        _git("commit", "--allow-empty", "-q", "-m", "init", cwd=repo)

        # add() creates a NEW branch in its own tree off HEAD.
        wt = await worktree.add("feature-x", repo=repo)
        assert wt.branch == "feature-x", wt
        assert wt.path.is_dir(), wt.path

        # os.PathLike + `wt / "x"` join onto the tree.
        assert os.fspath(wt) == str(wt.path), wt
        (wt / "hello.txt").write_text("hi")

        # list() is a DataFrame; exactly one tree is `current` (the main one), and
        # the new worktree is not it.
        lst = worktree.list(repo)
        assert isinstance(lst, pl.DataFrame) and "current" in lst.columns, lst.columns
        assert "feature-x" in set(lst["branch"].to_list()), lst
        assert lst.filter(pl.col("current")).height == 1, lst
        assert not lst.filter(pl.col("branch") == "feature-x")["current"][0], lst

        # commit() stages the new (untracked) file, so it lands in the commit.
        c = await wt.commit("add hello")
        assert c.ok, c.text
        tracked = subprocess.run(
            ["git", "-C", str(wt.path), "ls-files"], capture_output=True, text=True
        ).stdout
        assert "hello.txt" in tracked, tracked

        # An existing branch is checked out (not recreated) by add().
        _git("branch", "existing", cwd=repo)
        wt2 = await worktree.add("existing", repo=repo)
        assert wt2.branch == "existing", wt2

        # remove() drops the tree (force discards uncommitted changes in it);
        # main + feature-x remain.
        rm = await wt2.remove(force=True)
        assert rm.ok, rm.text
        assert worktree.list(repo).height == 2, worktree.list(repo)

        print("worktree-ok")


    asyncio.run(main())
  '';
  worktreeSmoke =
    pkgs.runCommand "ix-mcp-worktree-smoke"
      {
        nativeBuildInputs = [
          mcpPython
          pkgs.git
        ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"
        ${lib.getExe mcpPython} ${worktreeTestPy} >stdout 2>stderr || {
          echo "ix-mcp worktree smoke failed:" >&2
          cat stdout stderr >&2
          exit 1
        }
        grep -qx 'worktree-ok' stdout || {
          echo "ix-mcp worktree smoke did not confirm the worktree module:" >&2
          cat stdout stderr >&2
          exit 1
        }
        mkdir -p "$out"
      '';

  screenBundled = importTest "screen" "import screen; print('screen-ok', all(callable(getattr(screen, n)) for n in ('capture', 'click', 'write', 'press', 'key_down', 'key_up', 'apps', 'frontmost', 'launch', 'activate', 'terminate', 'accessibility_trusted')))";
  vmkitBundled = importTest "vmkit" "import vmkit; print('vmkit-ok', callable(vmkit.boot_linux), callable(vmkit.drive), callable(vmkit.screenshot))";
in
package.overrideAttrs (old: {
  passthru = (old.passthru or { }) // {
    tests = {
      inherit
        tuiBundled
        htpyBundled
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
        wedgeSmoke
        richSmoke
        yieldSmoke
        bindingsSmoke
        bindDefaultSmoke
        viewSmoke
        nixSmoke
        fleetSmoke
        shSmoke
        worktreeSmoke
        ;
      site = dashboardSite;
    }
    // lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
      inherit screenBundled vmkitBundled vmkitResourceSmoke;
    };
  };
})
