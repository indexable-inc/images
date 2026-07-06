{
  ix,
  lib,
}: let
  # The headless Nix build-tree emitter. The `nix` module's live-pane path spawns
  # it (`nix-web-monitor --emit ndjson`) so the parser stays the single owner of
  # internal-json; baked onto the wrapper env (IX_NIX_WEB_MONITOR_BIN) rather than
  # resolved from PATH. It rides the ix overlay (`overlay = true` in its
  # package.nix), so it is on the overlaid `pkgs` under its id -- read it there,
  # not via a `repoPackages` formal, because mcp is also called through
  # callPackage paths (e.g. pi-harness) that do not bind one.
  nixWebMonitorBin = pkgs.nix-web-monitor;
  # Read the package set from `ix` rather than a `pkgs` callPackage formal (which
  # `override` can't reach). `ix.pkgs` is the caller's set, the same value
  # callPackage would have auto-bound to a `pkgs` arg in the flake package set.
  inherit (ix) pkgs;

  # PyPI source pins (version + sdist URL + SRI hash) for the interpreter
  # overrides below, in the sibling pins.json (repo policy: no inline hash
  # literals in tracked .nix). Each `url` is fetchPypi's canonical pypi.io
  # source path (verified byte-identical to the pinned hashes). Re-pin after a
  # version edit manually (rebuild, copy the `got:` hash): mcp carries no
  # registry updateScript, so `nix run .#update` does not touch these pins.
  pypiPins = ix.pins.loadPins ./pins.json;
  # The PTY-driving `tui` package, baked into the pinned interpreter so every
  # session can `import tui` with no setup. The PyO3 cdylib comes from the same
  # shared workspace graph the binary is selected from, dropped next to the
  # package's Python source as the `tui._tui` extension. This is the cdylib
  # straight from the graph rather than the distributable wheel, so it also
  # works on macOS, where the wheel packaging stays Linux-only. Store references
  # in the cdylib are fine: this module never leaves the Nix environment.
  tuiPythonSource = builtins.path {
    name = "tui-py-python-source";
    path = ix.paths.packagesRoot + "/tui/tui-py/python";
  };
  tuiModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-tui-python-module"
    {
      strictDeps = true;
      propagatedBuildInputs = [pkgs.python3.pkgs.numpy];
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
    path = ix.paths.packagesRoot + "/search/search-py/python";
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

  # The embedded nushell engine, baked into the pinned interpreter so every
  # session can `await nu("ls | where size > 1kb")` and get a polars frame.
  # Same shape as `searchModule`: the PyO3 cdylib comes from the shared
  # workspace graph (packages/nu-py), so it works on Linux and macOS dev alike.
  # In-process, not a subprocess: one persistent engine per kernel, cancellable
  # through nushell's own interrupt signal.
  nuPyPythonSource = builtins.path {
    name = "nu-py-python-source";
    path = ix.paths.packagesRoot + "/nu-py/python";
  };
  nuPyModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-nu-python-module"
    {
      strictDeps = true;
      meta.description = "Embedded nushell engine (nu-py PyO3 module) bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/nu"
      mkdir -p "$site"
      cp -r ${nuPyPythonSource}/nu/. "$site/"

      cdylib=""
      for candidate in \
        ${ix.rustWorkspace.units.libraries.nu_py}/lib/libnu_py.so \
        ${ix.rustWorkspace.units.libraries.nu_py}/lib/libnu_py-*.so \
        ${ix.rustWorkspace.units.libraries.nu_py}/lib/libnu_py.dylib \
        ${ix.rustWorkspace.units.libraries.nu_py}/lib/libnu_py-*.dylib
      do
        if [ -f "$candidate" ]; then
          cdylib="$candidate"
          break
        fi
      done
      if [ -z "$cdylib" ]; then
        echo "nu-py module: no cdylib under ${ix.rustWorkspace.units.libraries.nu_py}/lib" >&2
        ls -la ${ix.rustWorkspace.units.libraries.nu_py}/lib >&2 || true
        exit 1
      fi
      install -m555 "$cdylib" "$site/_nu.abi3.so"
    ''
  );

  # The astlog package, baked into the pinned interpreter so every session can
  # `import astlog` and run Datalog queries/rewrites over tree-sitter ASTs with
  # no setup. Same shape as `searchModule`: the PyO3 cdylib comes from the
  # shared workspace graph, so it works on Linux and macOS dev alike.
  astlogPythonSource = builtins.path {
    name = "astlog-py-python-source";
    path = ix.paths.packagesRoot + "/code/astlog/py/python";
  };
  astlogModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-astlog-python-module"
    {
      strictDeps = true;
      meta.description = "astlog PyO3 module bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/astlog"
      mkdir -p "$site"
      cp -r ${astlogPythonSource}/astlog/. "$site/"

      cdylib=""
      for candidate in \
        ${ix.rustWorkspace.units.libraries.astlog_py}/lib/libastlog_py.so \
        ${ix.rustWorkspace.units.libraries.astlog_py}/lib/libastlog_py-*.so \
        ${ix.rustWorkspace.units.libraries.astlog_py}/lib/libastlog_py.dylib \
        ${ix.rustWorkspace.units.libraries.astlog_py}/lib/libastlog_py-*.dylib
      do
        if [ -f "$candidate" ]; then
          cdylib="$candidate"
          break
        fi
      done
      if [ -z "$cdylib" ]; then
        echo "ix-astlog module: no cdylib under ${ix.rustWorkspace.units.libraries.astlog_py}/lib" >&2
        ls -la ${ix.rustWorkspace.units.libraries.astlog_py}/lib >&2 || true
        exit 1
      fi
      install -m555 "$cdylib" "$site/_astlog.abi3.so"
    ''
  );

  # The scipql package, baked into the pinned interpreter so every session can
  # `import scipql` and run Soufflé datalog + find/replace over a SCIP semantic
  # index. Same shape as `astlogModule`: the PyO3 cdylib comes from the shared
  # workspace graph. (The CLI bakes in rust-analyzer/souffle; the kernel module
  # exposes facts/query/fix/rename over an already-built index.scip.)
  scipqlPythonSource = builtins.path {
    name = "scipql-py-python-source";
    path = ix.paths.packagesRoot + "/code/scipql/py/python";
  };
  scipqlModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-scipql-python-module"
    {
      strictDeps = true;
      meta.description = "scipql PyO3 module bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/scipql"
      mkdir -p "$site"
      cp -r ${scipqlPythonSource}/scipql/. "$site/"

      cdylib=""
      for candidate in \
        ${ix.rustWorkspace.units.libraries.scipql_py}/lib/libscipql_py.so \
        ${ix.rustWorkspace.units.libraries.scipql_py}/lib/libscipql_py-*.so \
        ${ix.rustWorkspace.units.libraries.scipql_py}/lib/libscipql_py.dylib \
        ${ix.rustWorkspace.units.libraries.scipql_py}/lib/libscipql_py-*.dylib
      do
        if [ -f "$candidate" ]; then
          cdylib="$candidate"
          break
        fi
      done
      if [ -z "$cdylib" ]; then
        echo "ix-scipql module: no cdylib under ${ix.rustWorkspace.units.libraries.scipql_py}/lib" >&2
        ls -la ${ix.rustWorkspace.units.libraries.scipql_py}/lib >&2 || true
        exit 1
      fi
      install -m555 "$cdylib" "$site/_scipql.abi3.so"
    ''
  );

  # The flecs-query package, baked into the pinned interpreter so every
  # session can `import flecs_query` and parse/validate Flecs Query Language
  # expressions with no setup. Same shape as `astlogModule`: the PyO3 cdylib
  # comes from the shared workspace graph, so it works on Linux and macOS dev
  # alike.
  flecsQueryPythonSource = builtins.path {
    name = "flecs-query-py-python-source";
    path = ix.paths.packagesRoot + "/flecs-query/py/python";
  };
  flecsQueryModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-flecs-query-python-module"
    {
      strictDeps = true;
      meta.description = "flecs-query PyO3 module bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/flecs_query"
      mkdir -p "$site"
      cp -r ${flecsQueryPythonSource}/flecs_query/. "$site/"

      cdylib=""
      for candidate in \
        ${ix.rustWorkspace.units.libraries.flecs_query_py}/lib/libflecs_query_py.so \
        ${ix.rustWorkspace.units.libraries.flecs_query_py}/lib/libflecs_query_py-*.so \
        ${ix.rustWorkspace.units.libraries.flecs_query_py}/lib/libflecs_query_py.dylib \
        ${ix.rustWorkspace.units.libraries.flecs_query_py}/lib/libflecs_query_py-*.dylib
      do
        if [ -f "$candidate" ]; then
          cdylib="$candidate"
          break
        fi
      done
      if [ -z "$cdylib" ]; then
        echo "ix-flecs-query module: no cdylib under ${ix.rustWorkspace.units.libraries.flecs_query_py}/lib" >&2
        ls -la ${ix.rustWorkspace.units.libraries.flecs_query_py}/lib >&2 || true
        exit 1
      fi
      install -m555 "$cdylib" "$site/_flecs_query.abi3.so"
    ''
  );

  # The `fsearch` filesystem-search module: `grep`/`find`/`spotlight`, each
  # backed by a battle-tested CLI (ripgrep / fd / macOS Spotlight) run as a
  # SEPARATE process via the kernel-private `sh._exec` runner, returning polars
  # frames. Pure Python over the sh runner/polars; cross-platform (spotlight is
  # darwin-only and guards itself). Unlike its predecessor `fff` (a ctypes cdylib
  # that walked the tree in-process and could pin the cores for an hour with no
  # way to interrupt short of killing the kernel), a runaway here is
  # process-isolated and bounded by `_exec`'s timeout + process-group kill.
  # `ripgrep`/`fd` are put on the interpreter wrapper's PATH below so the runner
  # resolves them.
  fsearchPythonSource = builtins.path {
    name = "ix-mcp-fsearch-python-source";
    path = ./src/fsearch;
  };
  fsearchModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-fsearch-python-module"
    {
      strictDeps = true;
      meta.description = "rg/fd/Spotlight-backed grep/find/spotlight bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/fsearch"
      mkdir -p "$site"
      cp -r ${fsearchPythonSource}/fsearch/. "$site/"
    ''
  );

  # The `ix_google` package: typed PyO3 bindings for the google-gmail and
  # google-calendar Rust crates, baked into the pinned interpreter as a
  # complement to the (untyped) `google_auth` helper. Notebook users pick
  # whichever fits: `import google_auth` gives the official googleapiclient
  # surface, `import ix_google` gives typed `gmail.Client()` /
  # `calendar.Client()` over the same shared OAuth grant. Sign-in is
  # self-service from a session (`await google_auth.login()` opens a browser),
  # or `gmail auth` / `gcal auth` on the host.
  ixGooglePythonSource = builtins.path {
    name = "ix-google-python-source";
    path = ix.paths.packagesRoot + "/google/py/python";
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

  # `google_auth`: Gmail + Calendar for the kernel, with self-service sign-in.
  # Pure Python (no cdylib): it shells to the bundled `gcal` binary
  # (`IX_GCAL_BIN`, set on the wrapper below) to sign in (`login()` drives
  # `gcal auth --json` and opens a browser), to sign out (`logout()`), and to
  # mint short-lived access tokens from the shared grant, which it wraps as a
  # `google.oauth2.credentials` object the official client accepts. The refresh
  # token / client secret stay inside `gcal`; only access tokens cross into
  # Python.
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
  # Python over the bundled polars/pygments; cross-platform, so every session
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
  # Tailnet mesh discovery (index#1787): `await mesh.peers()` sweeps tailscale
  # peers for live ix-mcp `/mesh` endpoints (served by ix_notebook_mcp.mesh on
  # the well-known mesh port) and returns one polars row per responding server;
  # `mesh.sessions()` flattens to (host, session). Pure Python over the bundled
  # httpx + polars; cross-platform.
  meshPythonSource = builtins.path {
    name = "ix-mcp-mesh-python-source";
    path = ./src/mesh;
  };
  meshModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-mesh-python-module"
    {
      strictDeps = true;
      meta.description = "Tailnet mesh discovery of live ix-mcp servers, bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/mesh"
      mkdir -p "$site"
      cp -r ${meshPythonSource}/mesh/. "$site/"
    ''
  );
  # The kernel's process runner. The public `sh()`/`zsh()` are RETIRED (agents
  # shell out through `await nu(...)`); they stay importable as disabled shims
  # that raise a migration hint. The private `sh._exec` runs on the kernel's loop
  # (never blocks it like a bare subprocess.run) and returns an Output that IS a
  # Result, so the dashboard sees the command's ANSI color rendered to HTML while
  # the model gets the same text escape-stripped. Kernel internals (the
  # grep/find search helpers, worktree plumbing) use `_exec`. Pure Python over the
  # bundled ansi2html; cross-platform.
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
  # Svelte 5 components as live interactive resources: `import svelte`, then
  # `await svelte.component("Board.svelte", id=..., actions=...)` compiles via
  # the svelte-bundle CLI and registers the result, with the virtual `ix`
  # module (`data`/`act`/`replies`) wired to the resource event feed. Pure
  # Python; the compiler is the wrapped Node CLI above.
  sveltePythonSource = builtins.path {
    name = "ix-mcp-svelte-python-source";
    path = ./src/svelte;
  };
  svelteModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-svelte-python-module"
    {
      strictDeps = true;
      meta.description = "Svelte 5 resource components bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/svelte"
      mkdir -p "$site"
      cp -r ${sveltePythonSource}/svelte/. "$site/"
    ''
  );
  # Browser automation over CDP: `import browser`, then `await browser.goto(url)`
  # / `await browser.shot()` drive a Chromium-family browser already running with
  # --remote-debugging-port (the standard 9222 by default). Pure Python over the
  # bundled playwright (already in this interpreter, so no `pip`/`playwright
  # install`); runs on the kernel loop and returns the raw Playwright objects plus
  # a screenshot Result. Cross-platform.
  browserPythonSource = builtins.path {
    name = "ix-mcp-browser-python-source";
    path = ./src/browser;
  };
  browserModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-browser-python-module"
    {
      strictDeps = true;
      meta.description = "Playwright-over-CDP browser helper bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/browser"
      mkdir -p "$site"
      cp -r ${browserPythonSource}/browser/. "$site/"
    ''
  );
  # Read recent X (Twitter) posts into polars by driving the logged-in browser:
  # `import x`, then `await x.posts("@handle")` / `x.posts("home")` navigates the
  # browser `browser` connects to, scrolls until it has enough tweets, and parses
  # them into a polars frame. Pure Python over the bundled browser/playwright/polars
  # (X has no usable unauthenticated read API); cross-platform.
  xPythonSource = builtins.path {
    name = "ix-mcp-x-python-source";
    path = ./src/x;
  };
  xModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-x-python-module"
    {
      strictDeps = true;
      meta.description = "Read recent X posts to polars via the logged-in browser, bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/x"
      mkdir -p "$site"
      cp -r ${xPythonSource}/x/. "$site/"
    ''
  );
  # Slack: read channels, messages, threads; send messages; search -- all per-user
  # with a self-service token flow. Pure Python over stdlib urllib + polars.
  # Per-user credential: SLACK_USER_TOKEN/SLACK_TOKEN env or ~/.config/slack/token
  # (mode 0600, written by slack.login(token)). Incognito sessions only (personal
  # workspace data never reaches a shared room). Cross-platform.
  slackPythonSource = builtins.path {
    name = "ix-mcp-slack-python-source";
    path = ./src/slack;
  };
  slackModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-slack-python-module"
    {
      strictDeps = true;
      meta.description = "Per-user Slack channels/messages/search bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/slack"
      mkdir -p "$site"
      cp -r ${slackPythonSource}/slack/. "$site/"
    ''
  );
  # Beeper: read chats and messages across every connected network, search, and
  # send -- a polars-shaped wrapper over the local Beeper Desktop HTTP API
  # (default http://localhost:23373). Pure Python over the bundled httpx + polars.
  # Per-user credential: BEEPER_ACCESS_TOKEN env or ~/.config/beeper/token
  # (mode 0600, written by beeper.login(token)). Incognito sessions only (personal
  # chats never reach a shared room). Cross-platform.
  beeperPythonSource = builtins.path {
    name = "ix-mcp-beeper-python-source";
    path = ./src/beeper;
  };
  beeperModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-beeper-python-module"
    {
      strictDeps = true;
      meta.description = "Per-user Beeper Desktop chats/messages/search/send bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/beeper"
      mkdir -p "$site"
      cp -r ${beeperPythonSource}/beeper/. "$site/"
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
  # Example task-dependency graphs generated in Python and stored in SQLite:
  # `import tasks`, then `tasks.seed("tasks.sqlite")` writes a ~100-node DAG and
  # `tasks.load(...)` / `tasks.frame(...)` read it back. The task-graph demo site
  # reads the same SQLite file. Pure stdlib (sqlite3) + lazy polars.
  tasksPythonSource = builtins.path {
    name = "ix-mcp-tasks-python-source";
    path = ./src/tasks;
  };
  tasksModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-tasks-python-module"
    {
      strictDeps = true;
      meta.description = "Task-graph SQLite helper bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/tasks"
      mkdir -p "$site"
      cp -r ${tasksPythonSource}/tasks/. "$site/"
    ''
  );
  # Drive the Ghostty terminal over its AppleScript dictionary (Ghostty 1.3.2+):
  # `import ghostty`, then `await ghostty.surfaces()` reads every open surface
  # (id/tty/pid/cwd/name) into polars and `await ghostty.close_me()` closes the
  # window this session runs in. Pure Python shelling `osascript` on the loop; no
  # native binding, so a plain toPythonModule like `worktree`/`tasks`. macOS-only
  # at import; bundled only in `darwinExtraPackages`.
  ghosttyPythonSource = builtins.path {
    name = "ix-mcp-ghostty-python-source";
    path = ./src/ghostty;
  };
  ghosttyModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-ghostty-python-module"
    {
      strictDeps = true;
      meta.description = "Ghostty AppleScript control bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/ghostty"
      mkdir -p "$site"
      cp -r ${ghosttyPythonSource}/ghostty/. "$site/"
    ''
  );
  # Linear issue-tracker GraphQL client: `import linear`, then
  # `await linear.issue("ENG-123")` / `issue_update` / `issue_create` /
  # `project_create`. Pure Python over the already-bundled httpx; reads
  # LINEAR_API_KEY from the environment at call time.
  linearPythonSource = builtins.path {
    name = "ix-mcp-linear-python-source";
    path = ./src/linear;
  };
  linearModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-linear-python-module"
    {
      strictDeps = true;
      meta.description = "Linear GraphQL client bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/linear"
      mkdir -p "$site"
      cp -r ${linearPythonSource}/linear/. "$site/"
    ''
  );
  # Notion REST client: `import notion`, then `await notion.search(query)` /
  # `page(id)` / `blocks(id)` / `db_query(id)` / `page_create` / `blocks_append`
  # / `page_update`. Pure Python over the already-bundled httpx + polars; reads
  # NOTION_API_KEY from the environment at call time. Cross-platform.
  notionPythonSource = builtins.path {
    name = "ix-mcp-notion-python-source";
    path = ./src/notion;
  };
  notionModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-notion-python-module"
    {
      strictDeps = true;
      meta.description = "Notion REST client bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/notion"
      mkdir -p "$site"
      cp -r ${notionPythonSource}/notion/. "$site/"
    ''
  );
  # `nox_autotriage`: nox-aware adapter that converts a nox conformance report
  # into linear.triage Findings and files them to Linear.  Depends on
  # linearModule (for linear.triage).  Entry point: python -m nox_autotriage.
  noxAutotriagePythonSource = builtins.path {
    name = "ix-mcp-nox-autotriage-python-source";
    path = ./src/nox_autotriage;
  };
  noxAutotriageModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-nox-autotriage-python-module"
    {
      strictDeps = true;
      meta.description = "nox conformance -> Linear triage adapter bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/nox_autotriage"
      mkdir -p "$site"
      cp -r ${noxAutotriagePythonSource}/nox_autotriage/. "$site/"
    ''
  );
  # `mcp_client`: connect to any Model Context Protocol server and call its tools
  # from the kernel. Pure Python over the already-bundled `mcp` SDK (no cdylib),
  # so it wraps the SDK's awkward `async with` transport/session context managers
  # in a persistent `Server` driven by one background task. `import mcp_client`,
  # then `await mcp_client.connect(url_or_command)`.
  mcpClientPythonSource = builtins.path {
    name = "ix-mcp-mcp-client-python-source";
    path = ./src/mcp_client;
  };
  mcpClientModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-mcp-client-python-module"
    {
      strictDeps = true;
      meta.description = "MCP client helper bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/mcp_client"
      mkdir -p "$site"
      cp -r ${mcpClientPythonSource}/mcp_client/. "$site/"
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
  # Native macOS iMessage access, bundled like `screen`/`vmkit` so every session
  # can `import imessage` on Darwin. Pure Python over the bundled sqlite3/polars
  # (plus Foundation's NSUnarchiver to decode the archived message text): it reads
  # the Messages and Contacts SQLite databases into polars frames, sends new
  # messages through the Messages app over AppleScript, and edits contacts
  # through the Contacts app over JXA (so edits sync to iCloud). macOS-only; the
  # module raises off Darwin.
  imessagePythonSource = builtins.path {
    name = "ix-mcp-imessage-python-source";
    path = ./src/imessage;
  };
  imessageModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-imessage-python-module"
    {
      strictDeps = true;
      meta.description = "Native macOS iMessage read-to-polars + send bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/imessage"
      mkdir -p "$site"
      cp -r ${imessagePythonSource}/imessage/. "$site/"
    ''
  );

  # The vmkit binary `vmkit` spawns. Darwin-only; referenced lazily so a Linux
  # mcp build never forces it.
  vmkitBin = ix.rustWorkspace.units.binaries."vmkit";

  # The gcal binary the calendar tools spawn with --json: the CLI surface of
  # the google-calendar crate (packages/google/calendar), so the MCP binding
  # carries no calendar logic of its own (RFC 0003).
  gcalBin = ix.rustWorkspace.units.binaries."gcal";

  # The Svelte 5 -> one-IIFE-bundle compiler the `svelte` module spawns
  # (IX_SVELTE_BUNDLE_BIN): esbuild + esbuild-svelte from the lockfile pin in
  # ./svelte-bundle, so resource components need no network at view time.
  svelteBundleBin = import ./svelte-bundle {inherit ix pkgs;};

  # `import CoreLocation` on Darwin: the pyobjc binding for Apple's Core Location
  # framework, so a session can read the Mac's current location with no install
  # step. nixpkgs ships only a curated subset of the pyobjc framework bindings and
  # CoreLocation is not one of them, but every binding lives in pyobjc-core's
  # monorepo src as a sibling subdir built by identical glue. So rather than
  # duplicate that glue, derive it from the packaged `pyobjc-framework-Quartz`
  # (same src, same version, same build/patch steps, same pyobjc-core + Cocoa
  # deps) and only retarget the source subdir and the import check. This tracks
  # any nixpkgs build fixes to Quartz automatically.
  coreLocationModule = pkgs.python3.pkgs.pyobjc-framework-Quartz.overridePythonAttrs (old: {
    pname = "pyobjc-framework-CoreLocation";
    sourceRoot = "${old.src.name}/pyobjc-framework-CoreLocation";
    pythonImportsCheck = ["CoreLocation"];
    meta =
      old.meta
      // {
        description = "PyObjC wrappers for the Core Location framework on macOS";
      };
  });

  # `import ScriptingBridge` on Darwin: the pyobjc binding for Apple's Scripting
  # Bridge, so a session can drive any scriptable macOS app (Things, Music,
  # Finder, ...) as native Objective-C objects — `SBApplication` — with no
  # AppleScript strings and no install step. nixpkgs omits this binding too, so
  # derive it from Quartz the same way as `coreLocationModule` above (same
  # monorepo src, only the source subdir and import check change).
  scriptingBridgeModule = pkgs.python3.pkgs.pyobjc-framework-Quartz.overridePythonAttrs (old: {
    pname = "pyobjc-framework-ScriptingBridge";
    sourceRoot = "${old.src.name}/pyobjc-framework-ScriptingBridge";
    pythonImportsCheck = ["ScriptingBridge"];
    meta =
      old.meta
      // {
        description = "PyObjC wrappers for the Scripting Bridge framework on macOS";
      };
  });

  # `import MapKit` on Darwin: the pyobjc binding for Apple's MapKit framework,
  # so a session can run `MKLocalSearch` place searches with no install step.
  # Derived from `pyobjc-framework-Quartz` the same way `coreLocationModule` is
  # (it is a sibling subdir in the same pyobjc source tree); MapKit's bindings
  # depend on both CoreLocation and Quartz, so those modules join its inputs:
  # the upstream wheel's METADATA requires `pyobjc-framework-quartz`, and
  # `pythonRuntimeDepsCheck` fails the build if it is not a propagated input
  # (the override renames the Quartz package to MapKit, so Quartz must be added
  # back explicitly rather than inherited).
  mapKitModule = pkgs.python3.pkgs.pyobjc-framework-Quartz.overridePythonAttrs (old: {
    pname = "pyobjc-framework-MapKit";
    sourceRoot = "${old.src.name}/pyobjc-framework-MapKit";
    pythonImportsCheck = ["MapKit"];
    propagatedBuildInputs =
      (old.propagatedBuildInputs or [])
      ++ [
        coreLocationModule
        pkgs.python3.pkgs.pyobjc-framework-Quartz
      ];
    meta =
      old.meta
      // {
        description = "PyObjC wrappers for the MapKit framework on macOS";
      };
  });

  # Native macOS places & geocoding: places near a point (MapKit `MKLocalSearch`)
  # and geocoding both ways (CoreLocation `CLGeocoder`), all returned as polars
  # frames. Pure Python over the bundled pyobjc CoreLocation/MapKit; its async
  # bridge drains the main run loop cooperatively so the frameworks' main-thread
  # completion handlers fire without wedging the kernel's event loop. macOS-only
  # (the module raises off Darwin).
  mapsPythonSource = builtins.path {
    name = "ix-mcp-maps-python-source";
    path = ./src/maps;
  };
  mapsModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-maps-python-module"
    {
      strictDeps = true;
      meta.description = "Native macOS maps/location (MapKit + CoreLocation) bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/maps"
      mkdir -p "$site"
      cp -r ${mapsPythonSource}/maps/. "$site/"
    ''
  );

  # The `screen` helper is macOS-only, so its dependencies join the interpreter
  # only on Darwin. `pyobjc-framework-Quartz` is the maintained CoreGraphics
  # binding the helper wraps; Pillow (already transitive via matplotlib) carries
  # the PIL image type capture returns. `coreLocationModule` adds the Core
  # Location binding so location reads work out of the box, and
  # `scriptingBridgeModule` the Scripting Bridge binding for app automation.
  darwinExtraPackages = ps:
    lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
      ps.pyobjc-framework-Quartz
      coreLocationModule
      scriptingBridgeModule
      mapKitModule
      mapsModule
      screenModule
      vmkitModule
      imessageModule
      ghosttyModule
    ];

  # htpy: build HTML in plain Python (`div(class_="x")[ ... ]`), auto-escaping
  # every text node and attribute via markupsafe. Bundled so a session — and the
  # `view` renderer — can compose dashboard HTML without hand-rolling f-strings,
  # which is exactly where escaping is forgotten (the dtype-header XSS this
  # package set just had to patch). Not in nixpkgs; pure Python, one dep
  # (markupsafe). https://htpy.dev
  htpyModule = let
    pname = "htpy";
    inherit (pypiPins.htpy) version;
  in
    pkgs.python3.pkgs.buildPythonPackage {
      inherit pname version;
      pyproject = true;
      src = pkgs.fetchPypi {
        inherit pname version;
        inherit (pypiPins.htpy) hash;
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
      dependencies =
        [
          pkgs.python3.pkgs.markupsafe
        ]
        ++ lib.optional (lib.versionOlder pkgs.python3.pythonVersion "3.13") pkgs.python3.pkgs.typing-extensions;
      pythonImportsCheck = ["htpy"];
      doCheck = false;
    };

  # cursor-sdk: Cursor's official Python SDK -- script the same agent that runs
  # in the Cursor IDE/CLI (local or cloud runtimes) from a session, e.g.
  # Composer as a cheap delegated codebase-search agent
  # (`from cursor_sdk import AsyncAgent`). Wheel-only on PyPI (the sdist is a
  # stub; each wheel bundles that platform's SDK bridge binary), so pins.json
  # carries one wheel per nix system. Its one runtime dep is the bundled httpx.
  # No credentials ship: the caller brings CURSOR_API_KEY or a logged-in
  # cursor-agent. License is Cursor's proprietary SDK beta license; the marker
  # is omitted for the same allowUnfree reason as the cursor-cli/claude-code
  # vendored binaries.
  cursorSdkModule = let
    pin =
      pypiPins."cursor_sdk-${pkgs.stdenv.hostPlatform.system}"
        or (throw "cursor-sdk: no pinned wheel for ${pkgs.stdenv.hostPlatform.system}");
  in
    pkgs.python3.pkgs.buildPythonPackage {
      pname = "cursor-sdk";
      inherit (pin) version;
      format = "wheel";
      src = pkgs.fetchurl {inherit (pin) url hash;};
      # The manylinux wheel's bridge binary needs its interpreter/rpaths
      # rewritten to run from the store on NixOS.
      nativeBuildInputs = lib.optional pkgs.stdenv.hostPlatform.isElf pkgs.autoPatchelfHook;
      buildInputs = lib.optionals pkgs.stdenv.hostPlatform.isLinux [
        pkgs.stdenv.cc.cc.lib
        pkgs.zlib
      ];
      dependencies = [pkgs.python3.pkgs.httpx];
      pythonImportsCheck = ["cursor_sdk"];
      doCheck = false;
    };

  # The Spark Connect client `fleet.spark()` drives, pinned to the cluster's Spark
  # version (3.5.x, via spark-hive + spark-gluten in services.ix-spark). A Connect
  # client MUST match the server's minor, and nixpkgs' default pyspark is 4.x, so
  # we pin our own 3.5.5. py4j stays the nixpkgs 0.10.9.9 (pyspark 3.5.5 pins
  # 0.10.9.7, but py4j is patch-stable and pinning a second copy would duplicate
  # it in the closure). The bundled JVM jars are stripped -- the Connect path is
  # pure gRPC and never starts a local JVM, so ~300 MB of jars would be dead
  # weight. pyarrow IS required (the client materializes results as Arrow), so it
  # is bundled here, with Spark, rather than for the whole interpreter's sake.
  pysparkConnect = pkgs.python3.pkgs.pyspark.overridePythonAttrs (old: {
    inherit (pypiPins.pyspark) version;
    src = pkgs.python3.pkgs.fetchPypi {
      pname = "pyspark";
      inherit (pypiPins.pyspark) version hash;
    };
    # pyspark 3.5.5 pins py4j==0.10.9.7 exactly; relax it so the patch-newer
    # nixpkgs py4j 0.10.9.9 satisfies the runtime-deps check.
    pythonRelaxDeps = ["py4j"];
    # Keep pyspark's own deps (py4j) and add the Spark Connect client stack.
    propagatedBuildInputs =
      (old.propagatedBuildInputs or [])
      ++ [
        pkgs.python3.pkgs.grpcio
        pkgs.python3.pkgs.grpcio-status
        pkgs.python3.pkgs.googleapis-common-protos
        pkgs.python3.pkgs.protobuf
        pkgs.python3.pkgs.pandas
        pkgs.python3.pkgs.pyarrow
        pkgs.python3.pkgs.numpy
      ];
    # Strip the bundled Spark/JVM jars: fleet.spark uses only the gRPC Connect
    # client, so the jars (and the local-JVM code paths that need them) are unused.
    postInstall =
      (old.postInstall or "")
      + ''
        rm -rf "$out/${pkgs.python3.sitePackages}/pyspark/jars"
      '';
    doCheck = false;
    pythonImportsCheck = [
      "pyspark"
      "pyspark.sql.connect"
    ];
  });

  # pymobiledevice3 9.27.0, the modern pure-python iDevice client the `iphone`
  # helper drives. nixpkgs pins 7.7.0, which predates iOS 17+ RemoteXPC tunnel
  # support maturing and cannot drive a current iOS 26 device; 9.27.0 can. It is
  # an override of the nixpkgs derivation (src bumped to the 9.27.0 sdist, plus
  # the handful of deps 9.x added) rather than a uv project, because the closure
  # has two sdist-only deps (hexdump, lzfse) that uv cannot build offline in the
  # sandbox — nixpkgs already ships both prebuilt, so the override reuses them and
  # every native dep nixpkgs has solved (qh3, cryptography, pillow, av, …). The
  # delta over 7.7.0 is five deps (asn1, pyimg4, pyiosbackup, prompt-toolkit,
  # defusedxml) plus av off Darwin, and a relaxed typer floor.
  #
  # asn1 is pinned to 2.8.0 across the whole mcp interpreter's package set. nixpkgs
  # ships asn1 3.3.0, whose Encoder/Decoder API shift is what marks pyimg4 0.8.8
  # `broken` there (its gate is `asn1 >= "3"`); 0.8.8 + asn1 2.x is the
  # combination verified to mount the Developer Disk Image on-device. pyimg4 is
  # pulled both directly and transitively (pymobiledevice3 -> ipsw-parser), so the
  # downgrade must be set-wide, not per-package — a `packageOverrides` interpreter
  # makes every consumer see the unbroken 2.8.0. Nothing else in the closure needs
  # asn1 3.x.
  mcpPythonInterp = pkgs.python3.override {
    self = mcpPythonInterp;
    packageOverrides = final: prev: {
      asn1 = prev.asn1.overridePythonAttrs (_: {
        inherit (pypiPins.asn1) version;
        src = pkgs.fetchPypi {
          pname = "asn1";
          inherit (pypiPins.asn1) version hash;
        };
      });
      # pymobiledevice3 9.27.0 needs ipsw-parser >= 1.6.0; nixpkgs pins 1.5.0.
      # Bump to 1.7.3 (the verified resolution). 1.7.x swaps its click dep for
      # typer, so add it (and relax the floor, since the set's typer is 0.24.0).
      ipsw-parser = prev.ipsw-parser.overridePythonAttrs (old: {
        inherit (pypiPins.ipsw_parser) version;
        src = pkgs.fetchPypi {
          pname = "ipsw_parser";
          inherit (pypiPins.ipsw_parser) version hash;
        };
        env =
          (old.env or {})
          // {
            SETUPTOOLS_SCM_PRETEND_VERSION = pypiPins.ipsw_parser.version;
          };
        dependencies = (old.dependencies or []) ++ [final.typer];
        pythonRelaxDeps = (old.pythonRelaxDeps or []) ++ ["typer"];
      });
    };
  };

  # pyiosbackup: read/decrypt iOS backups. Required by pymobiledevice3 9.27.0 and
  # absent from nixpkgs (the packaged `iosbackup` is an unrelated project). Pure
  # Python; all of its deps are already in the interpreter. Built from the
  # asn1-pinned set so it shares one consistent closure.
  pyiosbackupModule = let
    pname = "pyiosbackup";
    inherit (pypiPins.pyiosbackup) version;
  in
    mcpPythonInterp.pkgs.buildPythonPackage {
      inherit pname version;
      pyproject = true;
      src = pkgs.fetchPypi {
        inherit pname version;
        inherit (pypiPins.pyiosbackup) hash;
      };
      build-system = [mcpPythonInterp.pkgs.setuptools];
      dependencies = [
        mcpPythonInterp.pkgs.bpylist2
        mcpPythonInterp.pkgs.cryptography
        mcpPythonInterp.pkgs.packaging
        mcpPythonInterp.pkgs.construct
        mcpPythonInterp.pkgs.click
      ];
      pythonImportsCheck = ["pyiosbackup"];
      doCheck = false;
    };

  # The 9.27.0 override itself, built from the asn1-pinned set (so pyimg4 and
  # ipsw-parser resolve to the unbroken 2.8.0). Keeps the nixpkgs 7.7.0 dependency
  # set (native deps stay nixpkgs-built) and adds the new 9.x deps; typer is
  # relaxed because nixpkgs pins 0.24.0 while 9.27.0 floors at 0.25 (the CLI runs
  # on 0.24's surface, exercised by the import-smoke check). setuptools-scm reads
  # the version from the sdist's PKG-INFO, pinned so the build never needs a .git.
  # Upstream tests need a device, so checks are off.
  pymobiledevice3_927 = mcpPythonInterp.pkgs.pymobiledevice3.overridePythonAttrs (old: {
    inherit (pypiPins.pymobiledevice3) version;
    src = pkgs.fetchPypi {
      pname = "pymobiledevice3";
      inherit (pypiPins.pymobiledevice3) version hash;
    };
    env =
      (old.env or {})
      // {
        SETUPTOOLS_SCM_PRETEND_VERSION = pypiPins.pymobiledevice3.version;
      };
    dependencies =
      (old.dependencies or [])
      ++ [
        mcpPythonInterp.pkgs.asn1
        mcpPythonInterp.pkgs.pyimg4
        pyiosbackupModule
        mcpPythonInterp.pkgs.prompt-toolkit
        mcpPythonInterp.pkgs.defusedxml
      ]
      ++ lib.optional (!pkgs.stdenv.hostPlatform.isDarwin) mcpPythonInterp.pkgs.av;
    pythonRelaxDeps = ["typer"];
    doCheck = false;
  });

  # The `iphone` helper source, bundled like `screen`/`vmkit`/`imessage` so every
  # session can `import iphone`. Pure Python: it shells out to the bundled
  # `pymobiledevice3` console script (resolved next to the interpreter at runtime)
  # and returns device data as polars frames and screenshots as PIL images.
  # Cross-platform (USB + a root `tunneld` are what it needs, not macOS), so it
  # builds and import-checks on Linux CI too.
  iphonePythonSource = builtins.path {
    name = "ix-mcp-iphone-python-source";
    path = ./src/iphone;
  };
  iphoneModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-iphone-python-module"
    {
      strictDeps = true;
      meta.description = "USB iOS device control (pymobiledevice3) bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/iphone"
      mkdir -p "$site"
      cp -r ${iphonePythonSource}/iphone/. "$site/"
    ''
  );

  # The interpreter the wrapper pins. Sessions build their venv from this with
  # `--system-site-packages`, so `tui`, `search`, `exa_py`, numpy, polars
  # (incl. Postgres via psycopg + SQLAlchemy), duckdb, httpx, htpy, and playwright
  # are importable by default while an in-session `pip install` still writes to
  # the per-session venv.
  # The bundled-package set the pinned interpreter carries. Named so a sibling
  # interpreter (the vdom property-test runner below) can reuse the exact same
  # modules and only add its test deps, instead of duplicating the long list.
  mcpPythonPackages = ps:
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
      # pydantic (v2): the boundary parser for untrusted/JSON data. The bundled
      # `linear` and `google_auth` modules parse their GraphQL/CLI JSON responses
      # into typed models with it instead of threading untyped dicts. (The MCP SDK
      # pulls it transitively too, but linear/google_auth depend on it directly,
      # so declare it explicitly.)
      ps.pydantic
      # htpy: compose HTML in Python with automatic escaping (see the module
      # definition above). The preferred way to build any dashboard markup.
      htpyModule
      # exa-py: the official Exa (exa.ai) SDK, so a session can run neural web
      # search, get page contents, and `answer(...)` over the live web with no
      # install step (`from exa_py import Exa`). It is a thin client over the Exa
      # REST API. No key is bundled: the caller brings `EXA_API_KEY` (sourced
      # from rbw/op per the secrets split), e.g. `Exa(os.environ["EXA_API_KEY"])`.
      ps.exa-py
      # cursor-sdk: Cursor's official agent SDK (see the module definition
      # above) so a session can run local/cloud Cursor agents with no install
      # step.
      cursorSdkModule
      # Gmail / Google Workspace, the "third surface" for an integration alongside
      # the MCP binding and the index CLI (RFC 0003): a session can drive the
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
      # dill: serializes functions and classes defined in cells, which stdlib
      # pickle cannot -- the session-file namespace checkpoints
      # (runtime.__ix_snapshot / __ix_restore) depend on it to bring an agent's
      # helpers back instantly when a session file is reopened.
      ps.dill
      # ray: the distributed-execution engine the `fleet` module drives. One Ray
      # cluster spans the tailnet (a head node holds the GCS, the rest join as
      # workers, all bound to their Tailscale IPv4); `fleet.run`/`fleet.submit`
      # ship cloudpickled callables to it and the shared object store (Plasma,
      # zero-copy on-node, peer-to-peer transfer between nodes, spill-to-disk
      # under memory pressure) carries args and results. We use Ray rather than
      # reinvent Plasma/Arrow/refcount-GC. It bundles its own cloudpickle, so a
      # function defined in a cell ships by value without a separate serializer.
      # nixpkgs ray builds on aarch64-darwin + {aarch64,x86_64}-linux, the exact
      # platforms the fleet and dev boxes run, so it joins the pinned interpreter
      # like any other module.
      ps.ray
      # The Spark Connect client `fleet.spark()` drives (defined above): a 3.5.5
      # pyspark pinned to the services.ix-spark cluster's Spark, jars stripped,
      # carrying its Arrow/gRPC connect deps. Lets a cell open a SparkSession on
      # the cluster master with no local JVM.
      pysparkConnect
      # pypdf: extract text from a PDF in-kernel, so a downloaded file can be
      # read/searched without shelling out or falling back to a host tool. Pure
      # Python, small (`from pypdf import PdfReader`).
      ps.pypdf
      tuiModule
      searchModule
      nuPyModule
      astlogModule
      scipqlModule
      flecsQueryModule
      fsearchModule
      googleAuthModule
      ixGoogleModule
      ixNotebookMcpModule
      viewModule
      nixModule
      fleetModule
      meshModule
      shModule
      svelteModule
      worktreeModule
      browserModule
      xModule
      slackModule
      beeperModule
      tasksModule
      linearModule
      notionModule
      noxAutotriageModule
      mcpClientModule
      # pymobiledevice3 9.27.0 (defined above) + the `iphone` wrapper that drives
      # its CLI. The wrapper resolves the `pymobiledevice3` console script next to
      # the interpreter, so both ride in the same env. Cross-platform: a USB
      # iDevice + a root `tunneld` are what the developer commands need, not macOS,
      # so CI builds the whole closure and import-checks it on Linux too.
      pymobiledevice3_927
      iphoneModule
    ]
    ++ darwinExtraPackages ps;
  mcpPython = mcpPythonInterp.withPackages mcpPythonPackages;

  # Browser bundle that matches the playwright-driver the python package is
  # patched to use. Exposed to the worker through PLAYWRIGHT_BROWSERS_PATH on the
  # wrapper below so launched browsers resolve without a network download.
  playwrightBrowsers = pkgs.playwright-driver.browsers;

  # Headless Chromium fatally aborts the moment it needs a font but cannot load
  # any fontconfig config: Skia's FontConfigInterface backend hits a
  # `Not implemented` path (SkFontMgr_FontConfigInterface.cpp) and the renderer
  # dies, surfacing to Playwright as `TargetClosedError`. The Nix build sandbox
  # has no /etc/fonts and no fonts on disk, so the smoke tests below that launch
  # a real (headless) browser must point fontconfig at a generated config
  # carrying at least one real font family.
  fontsConf = pkgs.makeFontsConf {fontDirectories = [pkgs.dejavu_fonts];};

  # `ix-mcp` is just the pinned interpreter invoked on the bundled package's CLI.
  # Everything (the entrypoint, the one shared kernel, the data API) runs in this
  # one interpreter, so the bundled modules are all importable with no install step.
  # The human-facing dashboard is the shared Loro hub (the `dashboard` aggregator):
  # `ix-mcp serve` spawns it (IX_DASHBOARD_BIN) and publishes its runs/resources/
  # namespace to it as panes; the aiohttp server keeps only the read-only /api the
  # embedders poll. So there is no committed UI artifact and no Svelte build here.
  dashboardHubBin = ix.rustWorkspace.units.binaries."dashboard";

  # `ty` (astral-sh's Rust type checker) drives the per-cell static type check the
  # kernel runs before every `python_exec` cell (see ix_notebook_mcp/typecheck.py).
  # It is a nix-provided dependency baked onto the wrapper env (IX_MCP_TY_BIN), not
  # fetched at runtime, and it checks against `mcpPython` (IX_MCP_TY_PYTHON) so a
  # cell importing a bundled module resolves that module's real types.
  tyBin = lib.getExe pkgs.ty;

  package =
    pkgs.runCommand "ix-mcp"
    {
      nativeBuildInputs = [pkgs.makeWrapper];
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
        --set IX_SVELTE_BUNDLE_BIN ${lib.escapeShellArg (lib.getExe svelteBundleBin)} \
        --set IX_GCAL_BIN ${lib.escapeShellArg "${gcalBin}/bin/gcal"} \
        --set IX_DASHBOARD_BIN ${lib.escapeShellArg (lib.getExe' dashboardHubBin "dashboard")} \
        --set SCIPQL_SOUFFLE ${lib.escapeShellArg (lib.getExe' pkgs.souffle "souffle")} \
        --set IX_MCP_TY_BIN ${lib.escapeShellArg tyBin} \
        --set IX_MCP_TY_PYTHON ${lib.escapeShellArg mcpPython.interpreter} \
        --set IX_NIX_WEB_MONITOR_BIN ${lib.escapeShellArg (lib.getExe nixWebMonitorBin)} \
        --prefix PATH : ${
        lib.makeBinPath [
          pkgs.ripgrep
          pkgs.fd
        ]
      } \
        ${lib.optionalString pkgs.stdenv.hostPlatform.isDarwin "--set IX_VMKIT_BIN ${lib.escapeShellArg "${vmkitBin}/bin/vmkit"}"}
      # The notebook engine alone (kernel + dashboard + session file, no MCP
      # transport): the same interpreter and env, entered at the `notebook`
      # subcommand. Our jupyter-shaped serve; the MCP server is one client of it.
      makeWrapper ${lib.getExe mcpPython} $out/bin/ix-notebook \
        --add-flags "-m ix_notebook_mcp notebook" \
        --set IX_MCP_VERSION ${lib.escapeShellArg ix.rev} \
        --set PLAYWRIGHT_BROWSERS_PATH ${lib.escapeShellArg playwrightBrowsers} \
        --set IX_SVELTE_BUNDLE_BIN ${lib.escapeShellArg (lib.getExe svelteBundleBin)} \
        --set IX_GCAL_BIN ${lib.escapeShellArg "${gcalBin}/bin/gcal"} \
        --set IX_DASHBOARD_BIN ${lib.escapeShellArg (lib.getExe' dashboardHubBin "dashboard")} \
        --set SCIPQL_SOUFFLE ${lib.escapeShellArg (lib.getExe' pkgs.souffle "souffle")} \
        --set IX_MCP_TY_BIN ${lib.escapeShellArg tyBin} \
        --set IX_MCP_TY_PYTHON ${lib.escapeShellArg mcpPython.interpreter} \
        --set IX_NIX_WEB_MONITOR_BIN ${lib.escapeShellArg (lib.getExe nixWebMonitorBin)} \
        --prefix PATH : ${
        lib.makeBinPath [
          pkgs.ripgrep
          pkgs.fd
        ]
      } \
        ${lib.optionalString pkgs.stdenv.hostPlatform.isDarwin "--set IX_VMKIT_BIN ${lib.escapeShellArg "${vmkitBin}/bin/vmkit"}"}
    '';

  # Import a module in the pinned interpreter and assert a marker line. Used by
  # the bundled-module tests: the thing each guards is that the module is
  # importable in the very interpreter the kernels run on, which is a plain
  # interpreter import (no kernel, no network), so the build sandbox can prove it.
  importTest = name: code:
    pkgs.runCommand "ix-mcp-${name}"
    {
      nativeBuildInputs = [mcpPython];
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

  # Strict type-check gate (ENG-3131). Mirrors lib/build/uv-application.nix's
  # zuban+ruff phase, but this package has no uv project (it is plain source
  # copied into the pinned interpreter), so the check runs directly: `zuban check
  # --strict` for correctness + `zuban`'s disallow-untyped-defs, and `ruff check
  # --select ANN` for the explicit-annotation gate the type checker does not own.
  # `mcpPython` is passed as `--python-executable` so every bundled dependency
  # (polars, mcp, jupyter, ...) resolves exactly as it does at runtime.
  #
  # Scoped, not all-or-nothing: only the modules under `strictGreenModules` are
  # checked, so a module is migrated by adding its name here once its source is
  # fully annotated and clean. The whole `src/` tree is on MYPYPATH regardless,
  # so a checked module's first-party cross-imports (e.g. `x` -> `browser`,
  # `nox_autotriage` -> `linear`) resolve even before those deps are migrated.
  # The `ix_notebook_mcp` server package and the remaining `src/*` modules are
  # added here as they are brought up to strict.
  strictGreenModules = [
    "tasks"
    "x"
    "nix"
    "nox_autotriage"
    "linear"
    "notion"
    "google_auth"
    "slack"
    "beeper"
    "view"
    "worktree"
    "mesh"
  ];
  # The `ix_notebook_mcp` server package is migrated file-by-file (the package
  # as a whole is still ~200 errors from strict-clean, index#1902): each file
  # here is a check target inside the copied `ix_notebook_mcp/` tree. zuban
  # only reports errors in the named targets, so a listed file's imports of
  # still-unmigrated siblings do not drag their errors in.
  strictGreenServerFiles = [
    "tools.py"
  ];
  strictTypecheck = let
    # All src module package dirs go on MYPYPATH so first-party cross-imports
    # resolve; the green subset are the actual check targets.
    allSrcModules = builtins.attrNames (builtins.readDir ./src);
    mypypath = lib.concatMapStringsSep ":" (m: "src/${m}") allSrcModules;
    targets = lib.concatStringsSep " " (
      map (m: "src/${m}/${m}") strictGreenModules
      ++ map (f: "ix_notebook_mcp/${f}") strictGreenServerFiles
    );
  in
    pkgs.runCommand "ix-mcp-strict-typecheck"
    {
      nativeBuildInputs = [
        pkgs.zuban
        pkgs.ruff
        mcpPython
      ];
      strictDeps = true;
      meta.description = "zuban --strict + ruff ANN over the migrated ix-mcp Python sources";
    }
    ''
      cp -r ${ixNotebookMcpSource} ix_notebook_mcp
      cp -r ${./src} src
      cp ${./zuban.ini} zuban.ini
      chmod -R u+w ix_notebook_mcp src

      export MYPYPATH=${lib.escapeShellArg mypypath}:.
      echo "zuban check --strict over: ${toString strictGreenModules} + ix_notebook_mcp: ${toString strictGreenServerFiles}"
      zuban check --strict \
        --config-file zuban.ini \
        --python-executable ${mcpPython.interpreter} \
        --python-version ${pkgs.python3.pythonVersion} \
        --platform linux \
        ${targets}
      echo "ruff check (ANN + TID251 no-cast) over: ${toString strictGreenModules} + ix_notebook_mcp: ${toString strictGreenServerFiles}"
      ruff check ${ix.ruffAnnArgs} ${targets}

      mkdir -p "$out"
    '';

  tuiBundled = importTest "tui" "import tui; print('tui-ok', tui.__version__)";
  # htpy must import and auto-escape: a `<` in a text node comes out as `&lt;`.
  htpyBundled = importTest "htpy" "import htpy; print('htpy-ok' if '&lt;' in str(htpy.div['<']) else 'htpy-bad')";
  searchBundled = importTest "search" "import search; print('search-ok', search.__version__)";
  # The astlog surface is callable and its public API returns polars frames:
  # `scan`/`fixes`/`suppressed` are DataFrames and `query` a dict of them. A
  # trivial inline rules string against one temp file keeps it fast and offline;
  # an `astlog-ignore` comment exercises the suppression-listing path end to end.
  astlogBundled = importTest "astlog" ''
    import os, tempfile
    import polars as pl
    import astlog

    assert all(
        callable(getattr(astlog, n)) for n in ("query", "scan", "suppressed", "fixes", "fix")
    ), "astlog public functions must be callable"

    rules = '(rule (id x) (match rust "(identifier) @x"))\n(lint id warning "an identifier {x}")\n'
    work = tempfile.mkdtemp()
    with open(os.path.join(work, "s.rs"), "w") as fh:
        fh.write("fn main() { let v = ignored; } // astlog-ignore\n")

    relations = astlog.query(rules, [work])
    findings = astlog.scan(rules, [work])
    edits = astlog.fixes(rules, [work])
    suppressed = astlog.suppressed(rules, [work])
    assert isinstance(relations, dict) and all(
        isinstance(frame, pl.DataFrame) for frame in relations.values()
    ), "query must return a dict of DataFrames"
    assert isinstance(findings, pl.DataFrame), "scan must return a DataFrame"
    assert isinstance(edits, pl.DataFrame), "fixes must return a DataFrame"
    assert isinstance(suppressed, pl.DataFrame), "suppressed must return a DataFrame"
    assert {"commentLine", "commentText"} <= set(suppressed.columns), suppressed.columns
    assert suppressed.height > 0, "the astlog-ignore line must be reported as suppressed"
    print("astlog-ok", astlog.__version__)
  '';

  # End-to-end through the bundled `fsearch` module: plant a temp tree (with a
  # .gitignore'd file), then prove `grep` (ripgrep) and `find` (fd) return the
  # planted hits, respect .gitignore by default, and that `spotlight` raises its
  # darwin guard off macOS. Runs real rg/fd (on the check's PATH); pure local FS,
  # no network, so the build sandbox runs it.
  fsearchTestPy = pkgs.writeText "ix-mcp-fsearch-test.py" ''
    # python
    import asyncio
    import os
    import subprocess
    import sys
    import tempfile

    import polars as pl

    import fsearch

    root = tempfile.mkdtemp()
    os.makedirs(os.path.join(root, "src"))
    with open(os.path.join(root, "hello_world.txt"), "w") as fh:
        fh.write("greetings\nfind me on this line\n")
    with open(os.path.join(root, "src", "main.rs"), "w") as fh:
        fh.write('fn main() {\n    println!("find me on this line");\n}\n')
    # A .gitignore'd file must be skipped by default and surfaced with no_ignore.
    # ripgrep only honors .gitignore inside a git repo, so init one.
    with open(os.path.join(root, ".gitignore"), "w") as fh:
        fh.write("ignored.txt\n")
    with open(os.path.join(root, "ignored.txt"), "w") as fh:
        fh.write("find me on this line\n")
    subprocess.run(["git", "init", "-q", root], check=True)


    async def main() -> None:
        g = await fsearch.grep("find me on this line", root)
        assert isinstance(g, pl.DataFrame), type(g)
        assert list(g.columns) == ["path", "line_number", "col", "match", "line", "abs_offset"], g.columns
        files = set(g["path"].to_list())
        assert all(files), "a match row had an empty path (rg bytes field not decoded?)"
        assert any("hello_world" in f for f in files), files
        assert any("main.rs" in f for f in files), files
        assert not any("ignored.txt" in f for f in files), f"gitignore not respected: {files}"

        # no_ignore surfaces the ignored file; fixed treats the alternation literally.
        gi = await fsearch.grep("find me on this line", root, no_ignore=True)
        assert any("ignored.txt" in f for f in gi["path"].to_list()), gi["path"].to_list()
        plain = await fsearch.grep("greetings|fn main", root, fixed=True)
        assert plain.height == 0, "fixed=True must treat the alternation literally"

        f = await fsearch.find(ext="rs", root=root)
        assert isinstance(f, pl.DataFrame), type(f)
        assert list(f.columns) == ["path", "name", "type", "size", "mtime"], f.columns
        assert any(n == "main.rs" for n in f["name"].to_list()), f["name"].to_list()
        assert set(f["type"].to_list()) == {"file"}, f["type"].to_list()

        d = await fsearch.find(kind="dir", root=root)
        assert any(n == "src" for n in d["name"].to_list()), d["name"].to_list()

        # spotlight is darwin-only: it must raise a clear error elsewhere.
        if sys.platform != "darwin":
            try:
                await fsearch.spotlight("anything", root)
            except fsearch.FsearchError as exc:
                assert "macOS" in str(exc), exc
            else:
                raise AssertionError("spotlight should raise off macOS")

        # issue #1754 bug 3: limit= short-circuits and flags the partial scan.
        # Plant a tree with many matches so a small limit truncates it.
        big = tempfile.mkdtemp()
        for i in range(50):
            with open(os.path.join(big, f"f{i}.txt"), "w") as fh:
                fh.write("needle here\n" * 20)  # 20 matches per file, 1000 total
        capped = await fsearch.grep("needle", big, limit=5)
        assert isinstance(capped, pl.DataFrame), type(capped)  # still a usable frame
        assert isinstance(capped, fsearch.PartialFrame), "a capped scan must be a PartialFrame"
        assert capped.truncated is True
        assert capped.height == 5, capped.height
        assert "limit" in capped.reason, capped.reason
        assert "partial" in repr(capped).lower(), "the repr must surface truncation"

        # A full scan under the limit is a plain frame with no truncated flag.
        full = await fsearch.grep("needle", big, limit=100_000)
        assert full.height == 1000, full.height
        assert not isinstance(full, fsearch.PartialFrame)
        assert not hasattr(full, "truncated")

        # A timeout returns the matches found before the deadline, not nothing.
        # A tiny timeout over the big tree is very likely to trip; if the machine
        # is fast enough to finish, the assertion below tolerates a complete scan.
        timed = await fsearch.grep("needle", big, limit=10_000_000, timeout=0.001)
        if isinstance(timed, fsearch.PartialFrame):
            assert timed.truncated is True
            assert "timed out" in timed.reason, timed.reason

        print("fsearch-ok", fsearch.__version__)


    asyncio.run(main())
  '';
  # fsearch's grep/find run real ripgrep/fd, so the check needs them on PATH
  # (the same two added to the interpreter wrapper). A dedicated interpreter with
  # all bundled modules + the planted tree proves the helpers end to end in the
  # Linux sandbox; spotlight only asserts its darwin guard there.
  fsearchTestPython = mcpPythonInterp.withPackages mcpPythonPackages;
  fsearchBundled =
    pkgs.runCommand "ix-mcp-fsearch"
    {
      nativeBuildInputs = [
        fsearchTestPython
        pkgs.ripgrep
        pkgs.fd
        pkgs.git
      ];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe fsearchTestPython} ${fsearchTestPy} >stdout 2>stderr || {
        echo "ix-mcp fsearch test failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -q '^fsearch-ok' stdout || {
        echo "ix-mcp fsearch test did not print its ok marker:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
  dataLibsBundled = importTest "data-libs" (
    "import psycopg, sqlalchemy, duckdb, httpx; "
    + "from sqlalchemy import create_engine; create_engine('postgresql+psycopg://u@h/db'); "
    + "from pypdf import PdfReader; "
    + "print('data-libs-ok')"
  );
  gmailLibsBundled = importTest "gmail-libs" (
    "from googleapiclient.discovery import build; from google.oauth2.credentials import Credentials; "
    + "import google_auth_oauthlib, google_auth_httplib2; "
    + "build('gmail', 'v1', credentials=Credentials(token='x'), static_discovery=True); "
    + "print('gmail-libs-ok')"
  );
  cursorSdkBundled = importTest "cursor-sdk" (
    "import cursor_sdk; from cursor_sdk import AsyncAgent, AsyncClient; "
    + "assert callable(getattr(AsyncAgent, 'create', None)); "
    + "print('cursor-sdk-ok')"
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
    # Self-service sign-in surface: login() is awaitable, status()/logout() are
    # plain calls. These are what makes Gmail discoverable and usable with no
    # host-side setup file.
    import asyncio as _asyncio

    assert _asyncio.iscoroutinefunction(google_auth.login)
    assert callable(google_auth.status) and callable(google_auth.logout)

    # In a shared (multiplayer) room (IX_MCP_SHARED set) Gmail/Calendar are
    # refused before minting ever looks for the grant, so a personal mailbox
    # never reaches state other participants can see.
    os.environ["IX_MCP_SHARED"] = "1"
    os.environ["IX_GCAL_BIN"] = "/nonexistent/gcal"
    try:
        google_auth.credentials()
    except google_auth.GoogleAuthError as exc:
        assert "shared" in str(exc).lower(), exc
    else:
        raise SystemExit("expected GoogleAuthError in a shared room")

    # Incognito is the default: with IX_MCP_SHARED unset the gate passes, so
    # minting then fails on the missing binary instead -- proving the shared
    # gate is the only thing that blocked it above.
    os.environ.pop("IX_MCP_SHARED", None)
    os.environ.pop("IX_GCAL_BIN", None)
    try:
        google_auth.credentials()
    except google_auth.GoogleAuthError as exc:
        assert "IX_GCAL_BIN" in str(exc), exc
    else:
        raise SystemExit("expected GoogleAuthError when IX_GCAL_BIN is unset")

    # status() answers instead of raising: a not-signed-in session reports
    # signed_in=False so a caller can branch on it and offer login().
    state = google_auth.status()
    assert state["signed_in"] is False, state
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
  # The `slack` helper imports and exposes its public surface. A real API call
  # needs SLACK_USER_TOKEN + network, so the sandbox-safe assertions are: the
  # module imports, the public callables exist, an unconfigured session raises
  # SlackError with a helpful message, and IX_MCP_SHARED=1 refuses access.
  slackBundled = importTest "slack" ''
    import os

    import slack

    assert callable(slack.login) and callable(slack.logout) and callable(slack.status)
    import asyncio as _asyncio

    assert _asyncio.iscoroutinefunction(slack.channels)
    assert _asyncio.iscoroutinefunction(slack.dms)
    assert _asyncio.iscoroutinefunction(slack.messages)
    assert _asyncio.iscoroutinefunction(slack.thread)
    assert _asyncio.iscoroutinefunction(slack.send)
    assert _asyncio.iscoroutinefunction(slack.search)

    # In a shared (multiplayer) room Slack is refused before any network call,
    # so personal workspace data never reaches state other participants can see.
    os.environ["IX_MCP_SHARED"] = "1"
    try:
        _asyncio.run(slack.channels())
    except slack.SlackError as exc:
        assert "shared" in str(exc).lower(), exc
    else:
        raise SystemExit("expected SlackError in a shared room")

    # Incognito is the default: with IX_MCP_SHARED unset the shared guard
    # passes, so the next failure is a missing token -- proving the guard was
    # the only thing that blocked it above.
    os.environ.pop("IX_MCP_SHARED", None)
    # Ensure no token env vars or file are present.
    os.environ.pop("SLACK_USER_TOKEN", None)
    os.environ.pop("SLACK_TOKEN", None)
    try:
        _asyncio.run(slack.channels())
    except slack.SlackError as exc:
        assert "token" in str(exc).lower(), exc
    else:
        raise SystemExit("expected SlackError when no token is configured")

    # status() answers instead of raising when not configured.
    state = slack.status()
    assert state["configured"] is False, state
    print("slack-ok")
  '';
  # The `beeper` helper imports and exposes its public surface. A real API call
  # needs BEEPER_ACCESS_TOKEN + a running Beeper Desktop, so the sandbox-safe
  # assertions are: the module imports, the public callables exist, an
  # unconfigured session raises BeeperError naming the token, and IX_MCP_SHARED=1
  # refuses access.
  beeperBundled = importTest "beeper" ''
    import os

    import beeper

    assert callable(beeper.login) and callable(beeper.logout)
    import asyncio as _asyncio

    assert _asyncio.iscoroutinefunction(beeper.status)
    assert _asyncio.iscoroutinefunction(beeper.accounts)
    assert _asyncio.iscoroutinefunction(beeper.chats)
    assert _asyncio.iscoroutinefunction(beeper.messages)
    assert _asyncio.iscoroutinefunction(beeper.search)
    assert _asyncio.iscoroutinefunction(beeper.search_chats)
    assert _asyncio.iscoroutinefunction(beeper.send)

    # In a shared (multiplayer) room Beeper is refused before any network call,
    # so personal chats never reach state other participants can see.
    os.environ["IX_MCP_SHARED"] = "1"
    try:
        _asyncio.run(beeper.accounts())
    except beeper.BeeperError as exc:
        assert "shared" in str(exc).lower(), exc
    else:
        raise SystemExit("expected BeeperError in a shared room")

    # Incognito is the default: with IX_MCP_SHARED unset the shared guard
    # passes, so the next failure is a missing token -- proving the guard was
    # the only thing that blocked it above.
    os.environ.pop("IX_MCP_SHARED", None)
    os.environ.pop("BEEPER_ACCESS_TOKEN", None)
    try:
        _asyncio.run(beeper.accounts())
    except beeper.BeeperError as exc:
        assert "token" in str(exc).lower(), exc
    else:
        raise SystemExit("expected BeeperError when no token is configured")

    # Regression: a datetime column whose every value is empty/missing must not
    # raise (polars format inference has no sample) -- _frame emits a typed null
    # column instead. A mixed column parses the real value and nulls the empty.
    allempty = beeper._frame([{"timestamp": ""}], {"timestamp": beeper._TS})
    assert allempty["timestamp"].dtype == beeper._TS, allempty.schema
    assert allempty["timestamp"].to_list() == [None], allempty
    mixed = beeper._frame(
        [{"timestamp": "2026-01-01T00:00:00Z"}, {"timestamp": ""}],
        {"timestamp": beeper._TS},
    )
    assert mixed["timestamp"].dtype == beeper._TS, mixed.schema
    assert mixed["timestamp"].null_count() == 1, mixed

    print("beeper-ok")
  '';

  # The requirements surface: local-only probes of every credential declared in
  # the registry. In the credential-less sandbox every probe must miss and the
  # remedies must be complete; planting a credential (env key, or the mgrep
  # token file) flips its line to naming the source. Also pins the registry's
  # slack declaration against the slack module's own constants, so the declared
  # probe can never drift from the resolution order the module actually uses.
  requirementsTestPy = pkgs.writeText "ix-mcp-requirements-test.py" ''
    # python
    import os
    from pathlib import Path

    import beeper
    import slack
    from ix_notebook_mcp import registry, requirements

    creds = dict(registry.credentialed())
    assert creds["slack"].env == tuple(slack._TOKEN_ENV_VARS), creds["slack"].env
    assert Path(creds["slack"].token_path).expanduser() == slack._TOKEN_FILE, creds["slack"].token_path
    assert creds["beeper"].env == tuple(beeper._TOKEN_ENV_VARS), creds["beeper"].env
    assert Path(creds["beeper"].token_path).expanduser() == beeper._TOKEN_FILE, creds["beeper"].token_path

    by_name = {s.name: s for s in requirements.statuses()}
    assert set(by_name) == set(creds), sorted(by_name)
    for name, status in by_name.items():
        assert status.satisfied_via is None, f"{name} unexpectedly satisfied via {status.satisfied_via}"
    for needle in ("MXBAI_API_KEY", "mixedbread.com", "mgrep login"):
        assert needle in by_name["search"].line, by_name["search"].line

    os.environ["EXA_API_KEY"] = "dummy-key-for-probe"
    token = Path.home() / ".mgrep" / "token.json"
    token.parent.mkdir(parents=True, exist_ok=True)
    token.write_text("{}")
    by_name = {s.name: s for s in requirements.statuses()}
    assert by_name["exa_py"].satisfied_via == "EXA_API_KEY", by_name["exa_py"]
    assert by_name["search"].satisfied_via == "token at ~/.mgrep/token.json", by_name["search"]
    assert "dummy-key-for-probe" not in by_name["exa_py"].line, by_name["exa_py"].line
    print("requirements-ok")
  '';
  requirementsSmoke =
    pkgs.runCommand "ix-mcp-requirements-smoke"
    {
      nativeBuildInputs = [
        package
        mcpPython
      ];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"

      # CLI contract in the credential-less sandbox: non-zero exit so setup
      # scripts can gate on it, with every remedy named on stdout.
      if ix-mcp requirements >stdout 2>stderr; then
        echo "ix-mcp requirements exited 0 without any credential:" >&2
        cat stdout stderr >&2
        exit 1
      fi
      for needle in MXBAI_API_KEY EXA_API_KEY LINEAR_API_KEY NOTION_API_KEY 'mgrep login'; do
        if ! grep -qF "$needle" stdout; then
          echo "requirements report is missing $needle:" >&2
          cat stdout stderr >&2
          exit 1
        fi
      done

      ${lib.getExe mcpPython} ${requirementsTestPy} >py-stdout 2>py-stderr || {
        echo "ix-mcp requirements smoke failed:" >&2
        cat py-stdout py-stderr >&2
        exit 1
      }
      grep -qx 'requirements-ok' py-stdout || {
        echo "requirements smoke did not print its ok marker:" >&2
        cat py-stdout py-stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
  engineBundled = importTest "engine" "import ipykernel, jupyter_client, nbformat, aiohttp, mcp; print('engine-ok')";

  # The server package imports and registers its full tool surface. Exercises the
  # FastMCP registration (schemas from type hints) without starting a kernel or
  # the Jupyter Server, so it is sandbox-safe.
  serverTools = importTest "server" (
    "import asyncio; from ix_notebook_mcp.tools import mcp; "
    + "names = sorted(t.name for t in asyncio.run(mcp.list_tools())); "
    # session_set_name joined the surface in #1615 but this expected set was
    # not updated with it; the stale drv kept passing from cache on main until
    # this package's inputs changed and forced a rebuild.
    + "expected = {'python_exec','pr_watch','read','kernel_trace','tui_act','session_set_name','topic_set','reply'}; "
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
      nativeBuildInputs = [package];
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
    # python
    from unittest.mock import patch
    from ix_notebook_mcp import cli

    status = {
        "BackendState": "Running",
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

    # Tailscale installed but stopped (or needs login): it still reports its
    # assigned IPs, but they are not bound to any interface, so the helper must
    # treat them as unusable and fall back to loopback.
    for state in ("Stopped", "NeedsLogin", "NoState"):
        stopped = {**status, "BackendState": state}
        with patch.object(cli, "_tailscale_status", return_value=stopped):
            assert cli._tailscale_ip() is None, f"{state}: expected None, got {cli._tailscale_ip()!r}"

    # No tailscale: the helpers return None so the CLI falls back to loopback.
    # Stubbing the inner _tailscale_status is more robust than juggling PATH or
    # the absolute fallback paths the real helper probes (which exist on hydra
    # outside the sandbox, so a PATH-only test would still find them).
    with patch.object(cli, "_tailscale_status", return_value=None):
        assert cli._tailscale_ip() is None, "expected None when tailscale is unavailable"
        assert cli._tailscale_dns_name() is None, "expected None when tailscale is unavailable"

    # IPv6-only or empty IP list: still None (the bind expects IPv4).
    with patch.object(
        cli,
        "_tailscale_status",
        return_value={"BackendState": "Running", "Self": {"TailscaleIPs": ["fd7a::1"]}},
    ):
        assert cli._tailscale_ip() is None, "IPv6-only TailscaleIPs should yield None"

    # _bindable: loopback is bindable; a reserved/unassigned address is not, so
    # the CLI falls back to loopback instead of crashing the dashboard.
    free = cli._free_port()
    assert cli._bindable("127.0.0.1", free) is True, "loopback must be bindable"
    assert cli._bindable("240.0.0.1", free) is False, "reserved address must be unbindable"

    print("bind-default-ok")
  '';
  bindDefaultSmoke =
    pkgs.runCommand "ix-mcp-bind-default-smoke"
    {
      nativeBuildInputs = [mcpPython];
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

  # Exercises _resolve_ssh_auth_sock: the helper must redirect SSH_AUTH_SOCK to
  # the 1Password agent on darwin when the Apple launchd socket (or no socket)
  # is present, must leave a custom non-Apple agent alone, and must always
  # return None on non-darwin platforms. Dependency-free pure Python, so the
  # build sandbox runs it.
  sshAuthSockTest = pkgs.writeText "ix-mcp-ssh-auth-sock-test.py" ''
    # python
    import os
    import tempfile
    from pathlib import Path

    from ix_notebook_mcp.cli import _resolve_ssh_auth_sock

    with tempfile.TemporaryDirectory() as tmp:
        home = Path(tmp)
        op_dir = home / "Library" / "Group Containers" / "2BUA8C4S2C.com.1password" / "t"
        op_dir.mkdir(parents=True)
        op_sock = op_dir / "agent.sock"
        op_sock.touch()

        # darwin + unset SSH_AUTH_SOCK -> forward to 1Password
        result = _resolve_ssh_auth_sock(None, home, "darwin", exists=os.path.exists)
        assert result == str(op_sock), f"expected op_sock, got {result!r}"

        # darwin + Apple launchd socket -> forward to 1Password
        apple = "/var/run/com.apple.launchd.XYZ123/Listeners"
        result = _resolve_ssh_auth_sock(apple, home, "darwin", exists=os.path.exists)
        assert result == str(op_sock), f"expected op_sock for apple agent, got {result!r}"

        # darwin + custom non-Apple agent -> do not override
        custom = "/run/user/1000/gnupg/S.gpg-agent.ssh"
        result = _resolve_ssh_auth_sock(custom, home, "darwin", exists=os.path.exists)
        assert result is None, f"must not clobber custom agent, got {result!r}"

        # non-darwin platform -> always None, even with op sock present
        for plat in ("linux", "win32"):
            result = _resolve_ssh_auth_sock(None, home, plat, exists=os.path.exists)
            assert result is None, f"expected None on {plat!r}, got {result!r}"
            result = _resolve_ssh_auth_sock(apple, home, plat, exists=os.path.exists)
            assert result is None, f"expected None on {plat!r} with apple sock, got {result!r}"

        # darwin but 1Password socket absent -> None (do not crash)
        missing_home = Path(tmp) / "missing"
        missing_home.mkdir()
        result = _resolve_ssh_auth_sock(None, missing_home, "darwin", exists=os.path.exists)
        assert result is None, f"expected None when op sock absent, got {result!r}"

    print("ssh-auth-sock-ok")
  '';
  sshAuthSockSmoke =
    pkgs.runCommand "ix-mcp-ssh-auth-sock-smoke"
    {
      nativeBuildInputs = [mcpPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${mcpPython}/bin/python3 ${sshAuthSockTest} >stdout 2>stderr || {
        echo "ix-mcp ssh-auth-sock smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'ssh-auth-sock-ok' stdout || {
        echo "ix-mcp ssh-auth-sock smoke did not confirm helper behaviour:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';

  # Exercises the shared-dashboard launcher logic: live_hub() ignores a missing
  # or stale (dead-port) hub-state file and accepts a live one, and the data
  # API's `/` landing page names `ix-mcp dashboard` instead of redirecting to a
  # dead hub port. This is the "no million dashboards" reuse contract plus the
  # dead-redirect fix, both pure Python (real loopback sockets, no `dashboard`
  # binary), so the sandbox runs it.
  dashboardLauncherTest = pkgs.writeText "ix-mcp-dashboard-launcher-test.py" ''
    # python
    import asyncio
    import json
    import os
    import socket
    import tempfile
    import threading
    import time
    from pathlib import Path

    from aiohttp.test_utils import TestClient, TestServer

    from ix_notebook_mcp import config, store
    from ix_notebook_mcp.dashboard import build_app, landing_html

    state = config.hub_state_path()

    # No state file -> no hub (and no socket probe even happens).
    state.unlink(missing_ok=True)
    assert config.live_hub() is None, "missing state must read as no hub"

    # Stale state: a record whose port has nothing listening is ignored.
    probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    probe.bind(("127.0.0.1", 0))
    dead = probe.getsockname()[1]
    probe.close()
    state.write_text(json.dumps({"pid": 1, "host": "0.0.0.0", "port": dead, "url": f"http://x:{dead}/"}))
    assert config.port_open(dead) is False, "closed port must not read as open"
    assert config.live_hub() is None, "stale state (dead port) must read as no hub"

    # Live state: a record whose port is accepting connections is reused.
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    # Backlog must exceed the probes below: each port_open leaves a completed but
    # un-accepted connection, so listen(1) would make the second probe time out.
    srv.listen(16)
    live = srv.getsockname()[1]
    url = f"http://join.example:{live}/"
    state.write_text(json.dumps({"pid": 1, "host": "0.0.0.0", "port": live, "url": url}))
    assert config.port_open(live) is True, "listening port must read as open"
    got = config.live_hub()
    assert got is not None and got["port"] == live and got["url"] == url, got

    # Dead pid: even with a live listener on the recorded port (port reuse by an
    # unrelated service), a dead recorded pid means the file is stale -> no hub.
    gone = os.fork()
    if gone == 0:
        os._exit(0)
    os.waitpid(gone, 0)  # reap so the pid is truly dead
    state.write_text(json.dumps({"pid": gone, "host": "127.0.0.1", "port": live, "url": url}))
    assert config.live_hub() is None, "stale state (dead pid) must read as no hub"
    srv.close()
    state.unlink(missing_ok=True)

    # _bind_ip hands the Rust hub a concrete, non-wildcard IP literal: IPs pass
    # through, names resolve, and a wildcard is refused (mapped to loopback) so the
    # board never binds every NIC.
    from ix_notebook_mcp import cli
    assert cli._bind_ip("127.0.0.1") == "127.0.0.1"
    assert cli._bind_ip("localhost") == "127.0.0.1"
    assert cli._bind_ip("0.0.0.0") == "127.0.0.1"  # noqa: S104 -- asserting the wildcard refusal
    assert cli._bind_ip("::") == "127.0.0.1"
    assert cli._bind_ip("::1") == "::1"  # a non-wildcard IPv6 literal passes through

    # _host_arg brackets IPv6 for the binary's host:port and the URL; IPv4/names
    # are returned raw (Python's own socket calls take the unbracketed host).
    assert cli._host_arg("127.0.0.1") == "127.0.0.1"
    assert cli._host_arg("::1") == "[::1]"

    # The data API landing page points at the command, never a bare redirect.
    html = landing_html()
    assert "ix-mcp dashboard" in html, html
    assert "/api/jobs" in html, html

    # A non-numeric IX_DASH_HUB_PORT must not crash the launcher: fall back to 8080.
    os.environ["IX_DASH_HUB_PORT"] = "not-a-port"
    assert cli._stable_hub_port() == 8080
    os.environ["IX_DASH_HUB_PORT"] = "9191"
    assert cli._stable_hub_port() == 9191
    os.environ.pop("IX_DASH_HUB_PORT")

    # Drive the real aiohttp `/` handler: 302 to a live hub, else the landing
    # page -- never the old dead redirect. Pins the off-loop probe too.
    async def check_index() -> None:
        conn = store.connect(os.path.join(tempfile.mkdtemp(), "s.db"))
        client = TestClient(TestServer(build_app(config.Config(workdir=Path(tempfile.mkdtemp())), conn)))
        await client.start_server()
        try:
            hub = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            hub.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            hub.bind(("127.0.0.1", 0))
            hub.listen(16)
            hub_port = hub.getsockname()[1]
            hub_url = f"http://127.0.0.1:{hub_port}/"
            state.write_text(json.dumps({"pid": 1, "host": "127.0.0.1", "port": hub_port, "url": hub_url}))
            resp = await client.get("/", allow_redirects=False)
            assert resp.status == 302 and resp.headers.get("Location") == hub_url, (
                resp.status,
                resp.headers.get("Location"),
            )
            hub.close()

            state.unlink(missing_ok=True)
            resp = await client.get("/", allow_redirects=False)
            assert resp.status == 200, resp.status
            assert "ix-mcp dashboard" in await resp.text()
        finally:
            await client.close()

    asyncio.run(check_index())

    # The auto-dashboard hub_port branch is gated on `auto_dashboard`: with it
    # ON, a live hub_port redirects; with it OFF (the default), a live listener on
    # hub_port must NOT redirect -- that port is reserved-but-unbound and could be
    # any unrelated process. Pins the wrong-redirect fix.
    async def check_auto_gate() -> None:
        listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind(("127.0.0.1", 0))
        listener.listen(16)
        hp = listener.getsockname()[1]
        try:
            auto = config.Config(
                workdir=Path(tempfile.mkdtemp()), host="127.0.0.1", advertised_host="127.0.0.1",
                hub_port=hp, auto_dashboard=True,
            )
            ca = TestClient(TestServer(build_app(auto, store.connect(os.path.join(tempfile.mkdtemp(), "a.db")))))
            await ca.start_server()
            try:
                r = await ca.get("/", allow_redirects=False)
                assert r.status == 302 and r.headers.get("Location") == auto.hub_url(), (r.status, r.headers.get("Location"))
            finally:
                await ca.close()

            noauto = config.Config(
                workdir=Path(tempfile.mkdtemp()), host="127.0.0.1", advertised_host="127.0.0.1",
                hub_port=hp, auto_dashboard=False,
            )
            cn = TestClient(TestServer(build_app(noauto, store.connect(os.path.join(tempfile.mkdtemp(), "n.db")))))
            await cn.start_server()
            try:
                r = await cn.get("/", allow_redirects=False)
                assert r.status == 200 and "ix-mcp dashboard" in await r.text(), r.status
            finally:
                await cn.close()
        finally:
            listener.close()

    asyncio.run(check_auto_gate())

    # Concurrent launches must spawn exactly one hub (the flock in _dashboard
    # serializes check-or-spawn): the loser blocks, then reuses the winner's
    # hub.json instead of starting a second hub.
    state.unlink(missing_ok=True)
    hub = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    hub.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    hub.bind(("127.0.0.1", 0))
    hub.listen(16)
    hub_port = hub.getsockname()[1]
    spawns = []

    def fake_spawn() -> dict:
        spawns.append(1)
        time.sleep(0.3)  # hold the lock so the racer is forced to wait on it
        st = {"pid": os.getpid(), "host": "127.0.0.1", "port": hub_port, "url": f"http://127.0.0.1:{hub_port}/"}
        config.hub_state_path().write_text(json.dumps(st))
        return st

    real_spawn = cli._spawn_shared_hub
    cli._spawn_shared_hub = fake_spawn
    try:
        threads = [threading.Thread(target=cli._dashboard, kwargs={"open_browser": False}) for _ in range(2)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
    finally:
        cli._spawn_shared_hub = real_spawn
        hub.close()
    assert len(spawns) == 1, f"expected exactly one spawn under the lock, got {len(spawns)}"
    state.unlink(missing_ok=True)

    print("dashboard-launcher-ok")
  '';
  dashboardLauncherSmoke =
    pkgs.runCommand "ix-mcp-dashboard-launcher-smoke"
    {
      nativeBuildInputs = [mcpPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${mcpPython}/bin/python3 ${dashboardLauncherTest} >stdout 2>stderr || {
        echo "ix-mcp dashboard-launcher smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'dashboard-launcher-ok' stdout || {
        echo "ix-mcp dashboard-launcher smoke did not confirm helper behaviour:" >&2
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
    # python
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
        # elapsed_s rides every summary so the per-call reply reports the run's cost
        assert isinstance(s["elapsed_s"], float) and s["elapsed_s"] >= 0.0, s
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

        # A print-only cell (last statement is None) returns its captured stdout,
        # so what it printed reaches the model -- a notebook's behavior.
        p = await run("print('hello-from-stdout')", budget=1.0, name="printed")
        assert p.status == "done", (p.status, p.error)
        assert isinstance(p.result, runtime.Result), type(p.result)
        assert "hello-from-stdout" in p.result.llm_result, p.result.llm_result
        # A silent side-effecting cell returns a quiet confirmation.
        q = await run("x_side_effect = 1", budget=1.0, name="silent")
        assert q.status == "done", (q.status, q.error)
        assert "done" in q.result.llm_result, q.result.llm_result

        # A bare final value that already renders richly is auto-wrapped in
        # Result.of, so `df` on the last line just works without an explicit Result.
        d = await run("import polars as pl\npl.DataFrame({'x': [1, 2]})", budget=2.0, name="auto-df")
        assert d.status == "done", (d.status, d.error)
        assert isinstance(d.result, runtime.Result), type(d.result)
        assert d.result.llm_result.startswith("shape: (2, 1) | x:Int64"), d.result.llm_result
        assert "[[x]; [1], [2]]" in d.result.llm_result, d.result.llm_result
        import json as _json
        import subprocess as _subprocess
        import tempfile as _tempfile
        from pathlib import Path as _Path

        _nuon_path = _Path(_tempfile.mkdtemp()) / "df.nuon"
        _nuon_path.write_text(d.result.llm_result.split("\n", 1)[1])
        _parsed = _json.loads(_subprocess.check_output(
            ["nu", "-c", f"open --raw {_nuon_path} | from nuon | to json -r"],
            text=True,
        ))
        assert _parsed == [{"x": 1}, {"x": 2}], _parsed

        class Split:
            def __ix_html__(self):
                return "<strong>human</strong>"
            def __ix_llm__(self):
                return {"answer": 42, "tags": ["nuon", "llm"]}

        split = runtime.Result.of(Split())
        assert split.user_html == "<strong>human</strong>", split.user_html
        _split_path = _Path(_tempfile.mkdtemp()) / "split.nuon"
        _split_path.write_text(split.llm_result)
        _split = _json.loads(_subprocess.check_output(
            ["nu", "-c", f"open --raw {_split_path} | from nuon | to json -r"],
            text=True,
        ))
        assert _split == {"answer": 42, "tags": ["nuon", "llm"]}, (split.llm_result, _split)
        from ix_notebook_mcp import outputs as _outputs_for_llm

        _bundle = split._repr_mimebundle_()
        assert runtime.IX_LLM_MIME in _bundle and _bundle["text/html"] == "<strong>human</strong>", _bundle
        _content = _outputs_for_llm.to_mcp([{"output_type": "display_data", "data": _bundle}])
        assert len(_content) == 1 and _content[0].text == split.llm_result, _content
        # Jupyter semantics: the last expression IS the result, whatever its type.
        sc = await run("1 + 1", budget=2.0, name="scalar")
        assert sc.status == "done", (sc.status, sc.error)
        assert "2" in sc.result.llm_result, sc.result.llm_result
        # ...and stdout printed along the way rides with a bare final value.
        both = await run("print('logged')\n40 + 2", budget=2.0, name="print-and-value")
        assert both.status == "done", (both.status, both.error)
        assert "logged" in both.result.llm_result and "42" in both.result.llm_result, (
            both.result.llm_result
        )

        # A cell ending in a failed process Output is loud on every surface a
        # watcher reads (issue #1766: a build dead on ENOSPC read as
        # still-compiling): the streamed stdout carries the failure line, so
        # paging a backgrounded job's .output/.tail() shows the terminal state,
        # and the result's model text leads AND ends with the exit marker. The
        # Output itself is falsy. (`sh` is retired; the runner is the private
        # `_exec` the kernel's own internals still use.)
        fsh = await run("from sh import _exec\nawait _exec('echo diag-line; exit 7')", budget=10.0, name="failed-exec")
        assert fsh.status == "done", (fsh.status, fsh.error)
        assert "diag-line" in fsh.output and "[exit 7]" in fsh.output, fsh.output
        assert fsh.result.llm_result.splitlines()[0].startswith("[exit 7]"), fsh.result.llm_result
        assert fsh.result.llm_result.rstrip().endswith("[exit 7]"), fsh.result.llm_result
        assert fsh.result.exit_code == 7 and not fsh.result.ok, fsh.result.exit_code
        assert bool(fsh.result) is False, "a failed Output must be falsy"

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

        # Job.wait: a timed wait that never raises -- one cell replaces a
        # sleep-and-poll loop. At a short deadline the job is still running;
        # with no deadline it returns the finished job.
        slow2 = await run("import asyncio\nawait asyncio.sleep(0.3)\nResult.text('w')", budget=0.02, name="wait")
        assert (await slow2.wait(0.01)).running(), slow2.status
        assert (await slow2.wait()).done() and slow2.result.llm_result == "w"

        # A Result nested inside a Result (llm_result=Result.text(...)) is
        # flattened to its model text at construction, so the summary/paging
        # path never hits a non-str ("Result object is not subscriptable").
        nested = runtime.Result(user_html="<b>x</b>", llm_result=runtime.Result.text("inner"))
        assert nested.llm_result == "inner", nested.llm_result
        nj = await run(
            "Result(user_html='<b>x</b>', llm_result=Result.text('inner-e2e'))",
            budget=2.0, name="nested",
        )
        assert nj.status == "done", (nj.status, nj.error)
        assert "inner-e2e" in nj.tail(100), nj.tail(100)
        assert runtime._job_summary(nj)["result_chars"] == len("inner-e2e")
        # Any other non-str llm_result coerces to its repr rather than crash later.
        odd = runtime.Result(user_html="x", llm_result=123)
        assert odd.llm_result == "123", odd.llm_result

        # __ix_read: a file-path target returns the file's CONTENTS to the model
        # (the dashboard note is user_html only), honoring start/end; and an
        # expression that EVALUATES to an existing path reads that file too.
        import pathlib
        import tempfile
        rd = ns["__ix_read"]
        p = pathlib.Path(tempfile.mkdtemp()) / "sample.txt"
        body = "alpha\nbeta\ngamma\ndelta"
        p.write_text(body)
        whole = await rd(str(p), None, None)
        assert whole.llm_result == body, repr(whole.llm_result)
        assert str(p) not in whole.llm_result or whole.llm_result == body
        span = await rd(str(p), 2, 3)
        assert span.llm_result == "beta\ngamma", repr(span.llm_result)
        ns["sample_path"] = str(p)
        via_expr = await rd("sample_path", None, None)
        assert via_expr.llm_result == body, repr(via_expr.llm_result)
        # A plain expression target still renders its value.
        ns["answer"] = 41
        via_val = await rd("answer + 1", None, None)
        assert via_val.llm_result == "42", repr(via_val.llm_result)

    asyncio.run(main())
    # api(): a discoverable catalog of kernel builtins + bundled modules. `nu`
    # is the catalogued shell-out path; the retired `sh` is NOT listed (though it
    # stays bound as a disabled shim so a stale call fails loudly, tested below).
    cat = ns["api"]()
    names = set(cat["name"].to_list())
    assert {"Result", "cells", "jobs", "nu", "api"} <= names, names
    assert "sh" not in names and "zsh" not in names, names
    filt = ns["api"]("cells")
    assert 1 <= filt.height <= cat.height, (filt.height, cat.height)

    # grep/find/spotlight (the fsearch search helpers) and view are pre-bound in
    # the namespace (no import needed), the way Result/cells/jobs are, so
    # `await grep(...)` / `view.tree(...)` just work.
    assert callable(ns.get("grep")) and callable(ns.get("find")), (ns.get("grep"), ns.get("find"))
    assert callable(ns.get("spotlight")), ns.get("spotlight")

    # `sh`/`zsh` stay bound but are DISABLED: calling either raises a migration
    # hint pointing at `await nu(...)`, so an old transcript fails loudly rather
    # than with a bare NameError.
    async def _sh_disabled() -> None:
        for expr in ("await sh('echo hi')", "await zsh('echo hi')", "await sh(['echo', 'hi'])"):
            r = await run(expr, budget=2.0, name="sh-disabled")
            assert r.status == "error", (expr, r.status)
            assert "await nu" in (r.error or ""), (expr, r.error)

    asyncio.run(_sh_disabled())
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

    # The dimension cap alone does not bound bytes: a busy 1280px screenshot stays
    # megabytes as PNG. So _fit_image_bytes also enforces _IMAGE_MAX_BYTES, falling
    # back to JPEG (and further downscales) -- a high-entropy image comes back well
    # under the byte cap instead of flooding the model's reply with base64.
    import os as _osr

    _noisy = _Image.frombytes("RGB", (3000, 1500), _osr.urandom(3000 * 1500 * 3))
    _nbuf = _io.BytesIO()
    _noisy.save(_nbuf, format="PNG")
    assert len(_nbuf.getvalue()) > runtime._IMAGE_MAX_BYTES, len(_nbuf.getvalue())
    _fit = runtime._coerce_image(_nbuf.getvalue())
    _raw = _b64.b64decode(_fit["data"])
    assert len(_raw) <= runtime._IMAGE_MAX_BYTES, ("over byte cap", len(_raw))
    _fw, _fh = _Image.open(_io.BytesIO(_raw)).size
    assert max(_fw, _fh) <= runtime._IMAGE_MAX_DIM, (_fw, _fh)
    # A small lossless image fits both caps and is kept byte-for-byte (a crisp PNG
    # for UI/diagrams is never needlessly re-encoded).
    _sbuf = _io.BytesIO()
    _Image.new("RGB", (200, 100), (10, 20, 30)).save(_sbuf, format="PNG")
    _small = _sbuf.getvalue()
    assert _b64.b64decode(runtime._coerce_image(_small)["data"]) == _small

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

    # outputs._image is the final byte net for every image reaching the model: a
    # small image becomes a real image block, but an oversize blob (e.g. a raw
    # display(fig) bundle that never went through the kernel's fitter) is dropped
    # with a short note rather than dumped as megabytes of base64.
    _ok = _outputs._image("image/png", _b64.b64encode(_small).decode("ascii"))
    assert _ok.type == "image", _ok
    _over = _b64.b64encode(b"x" * (_outputs.MAX_IMAGE_BYTES + 5000)).decode("ascii")
    _dropped = _outputs._image("image/png", _over)
    assert _dropped.type == "text" and "dropped" in _dropped.text, _dropped
    assert len(_dropped.text) < 400, len(_dropped.text)
    # End to end: an oversize image in a display bundle yields no giant text block.
    _rendered = _outputs.to_mcp([{"output_type": "display_data", "data": {"image/png": _over}}])
    assert all(_c.type == "text" for _c in _rendered), [_c.type for _c in _rendered]
    assert max(len(_c.text) for _c in _rendered) < 400, [len(_c.text) for _c in _rendered]

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
  # Locks the embed contract (ix_notebook_mcp/feed.py): the dashboard and the
  # room server both read the agent's presentation through `feed`, so prove a
  # snapshot returns running-pinned jobs with decoded rich outputs, the curated
  # cells and live resources, a change marker that advances as a running job
  # streams output, and that `feed.job` fetches one run by the id a python_exec
  # tool result names (and None for a miss).
  feedTestPy = pkgs.writeText "ix-mcp-feed-test.py" ''
    # python
    import tempfile
    import time

    from ix_notebook_mcp import feed, store

    conn = store.connect(tempfile.mktemp(suffix=".db"))
    now = time.time()
    store.start(conn, id="aa11", name="run1", code="Result.of(df)", started_at=now, budget=15.0)
    store.finish(
        conn, id="aa11", status="done", ended_at=now + 1, output="hi", result="42 rows",
        error=None, outputs=[{"data": {"text/html": "<table>x</table>"}}],
        bindings={"df": {"kind": "DataFrame"}},
    )
    store.start(conn, id="bb22", name="run2", code="time.sleep(99)", started_at=now + 2, budget=5.0)
    store.replace_cells(conn, [{"id": "cell0", "title": "latency", "position": 0,
                                "outputs": [{"data": {"text/html": "<b>p50</b>"}}]}])
    store.upsert_resource(conn, id="res0", title="term", kind="html", html="<pre>$</pre>",
                          status="live", created_at=now, updated_at=now)

    snap = feed.snapshot(conn)
    assert len(snap["jobs"]) == 2, snap["jobs"]
    assert snap["jobs"][0]["id"] == "bb22", "running job must pin first"
    done = snap["jobs"][1]
    assert done["outputs"][0]["data"]["text/html"] == "<table>x</table>", done
    assert done["bindings"] == {"df": {"kind": "DataFrame"}}, done
    assert snap["cells"][0]["outputs"][0]["data"]["text/html"] == "<b>p50</b>", snap["cells"]
    assert snap["resources"][0]["html"] == "<pre>$</pre>", snap["resources"]
    assert isinstance(snap["rev"], str), snap["rev"]

    one = feed.job(conn, "aa11")
    assert one is not None and one["result"] == "42 rows", one
    assert one["outputs"][0]["data"]["text/html"] == "<table>x</table>", one
    assert feed.job(conn, "nope") is None

    store.update_output(conn, "bb22", "tick tick tick")
    assert feed.snapshot(conn)["rev"] != snap["rev"], "rev must advance on streamed output"

    print("feed-ok")
  '';
  feedSmoke =
    pkgs.runCommand "ix-mcp-feed-smoke"
    {
      nativeBuildInputs = [mcpPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe mcpPython} ${feedTestPy} >stdout 2>stderr || {
        echo "ix-mcp feed smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'feed-ok' stdout || {
        echo "ix-mcp feed smoke did not confirm the embed contract:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';

  # The session identity feature: a run's session label flows kernel -> store ->
  # pane bridge so the dashboard can group and name each MCP client's runs.
  # Covers the store singleton row, runtime.Session's label precedence + store
  # mirror, and the reserved `__session__` pane the bridge publishes.
  sessionIdentityTestPy = pkgs.writeText "ix-mcp-session-identity-test.py" ''
    # python
    import tempfile

    from ix_notebook_mcp import pane_bridge, runtime, store

    conn = store.connect(tempfile.mktemp(suffix=".db"))

    # Store: the session row is a singleton (id 0) that round-trips and updates
    # in place rather than accumulating rows.
    assert store.get_session(conn) is None, "no session before it is set"
    store.set_session(conn, name="alpha", client="claude-code 2.1")
    got = store.get_session(conn)
    assert got["name"] == "alpha" and got["client"] == "claude-code 2.1", got
    store.set_session(conn, name="beta", client="claude-code 2.1")
    assert store.get_session(conn)["name"] == "beta", "set_session must update in place"
    assert conn.execute("SELECT count(*) FROM session").fetchone()[0] == 1, "singleton row"

    # runtime.Session: label precedence is explicit name > client . workdir.
    s = runtime.Session()
    s._workdir = "index"
    assert s.name == "index", s.name
    s._set_client("claude-code 2.1")
    assert s.name == "claude-code 2.1 · index", s.name
    s.name = "refactor auth"
    assert s.name == "refactor auth", s.name
    assert s.client == "claude-code 2.1", s.client

    # _sync mirrors the effective label to the store, and is a no-op when nothing
    # changed (so an idle session never rewrites the row).
    runtime._store = store
    runtime._store_conn = conn
    s._sync()
    assert store.get_session(conn)["name"] == "refactor auth", store.get_session(conn)
    stamp = store.get_session(conn)["updated_at"]
    s._sync()
    assert store.get_session(conn)["updated_at"] == stamp, "unchanged sync must not rewrite"

    # pane bridge: a reserved `__session__` data pane carries the label + client,
    # so the dashboard reads it for the session selector (and excludes it as a run).
    store.set_session(conn, name="my session", client="claude-code 2.1")
    panes = pane_bridge._panes(conn)
    sess = [p for p in panes if p["id"] == "__session__"]
    assert len(sess) == 1, panes
    pane = sess[0]
    assert pane["title"] == "my session", pane
    assert pane["view"]["kind"] == "data" and pane["view"]["renderer"] == "session", pane
    assert pane["view"]["data"]["client"] == "claude-code 2.1", pane

    print("session-identity-ok")
  '';

  sessionIdentitySmoke =
    pkgs.runCommand "ix-mcp-session-identity-smoke"
    {
      nativeBuildInputs = [mcpPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe mcpPython} ${sessionIdentityTestPy} >stdout 2>stderr || {
        echo "ix-mcp session identity smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'session-identity-ok' stdout || {
        echo "ix-mcp session identity smoke did not confirm the contract:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';

  # The read-only data API is also the embedding contract: a host (the room
  # server runs `ix-mcp` as its agent's only tool) reads the agent's rich
  # results back over HTTP and renders them in its own UI. Exercise that path
  # in-process: seed the store, start the dashboard server, and assert the JSON
  # routes (incl. the by-id lookup an embedder keys off the job id in a tool
  # reply) return the run's nbformat output bundles, cells, and live resources.
  apiTest = pkgs.writeText "ix-mcp-api-test.py" ''
    # python
    import asyncio, tempfile
    from pathlib import Path

    import aiohttp

    from ix_notebook_mcp import cli, dashboard, store
    from ix_notebook_mcp.config import Config, set_config

    # An embedder pins the data-API port so it knows where to reach this instance.
    import os

    os.environ["IX_MCP_DASHBOARD_PORT"] = "54321"
    assert cli._dashboard_port() == 54321, cli._dashboard_port()
    os.environ.pop("IX_MCP_DASHBOARD_PORT")
    assert isinstance(cli._dashboard_port(), int)

    tmp = Path(tempfile.mkdtemp())

    # An embedder pins the execution store the same way (the pi-harness room
    # event mapper polls exactly this file); unset, the store is minted in the
    # runtime dir keyed by the data-API port.
    os.environ["IX_MCP_STORE"] = str(tmp / "pinned-store.sqlite")
    assert cli._store_path(54321) == tmp / "pinned-store.sqlite", cli._store_path(54321)
    os.environ["IX_MCP_STORE"] = ""
    assert cli._store_path(54321).name == "store-54321.db", cli._store_path(54321)
    os.environ.pop("IX_MCP_STORE")
    assert cli._store_path(54321).name == "store-54321.db", cli._store_path(54321)
    store_path = tmp / "store.db"
    conn = store.connect(store_path)
    rich = [
        {
            "output_type": "execute_result",
            "data": {
                "text/plain": "shape: (1, 1)",
                "text/html": "<table><tr><td>1</td></tr></table>",
            },
        }
    ]
    store.start(conn, id="job1", name="demo", code="df.head()", started_at=1000.0, budget=15.0)
    store.finish(
        conn,
        id="job1",
        status="done",
        ended_at=1001.0,
        output="stdout tail",
        result="ok",
        error=None,
        outputs=rich,
        bindings={"df": {"summary": "DataFrame"}},
    )
    store.replace_cells(conn, [{"id": "c1", "title": "Result", "position": 0, "outputs": rich}])
    store.upsert_resource(
        conn, id="r1", title="Live", kind="html", html="<b>hi</b>", status="live",
        created_at=1000.0, updated_at=1000.0,
    )

    cfg = Config(
        workdir=tmp, host="127.0.0.1", advertised_host="127.0.0.1",
        dashboard_port=cli._free_port(), store_path=store_path,
    )
    set_config(cfg)

    async def main():
        runner = await dashboard.start(cfg)
        base = f"http://127.0.0.1:{cfg.dashboard_port}"
        try:
            async with aiohttp.ClientSession() as session:
                async with session.get(base + "/api/jobs") as resp:
                    jobs = await resp.json()
                assert len(jobs) == 1 and jobs[0]["id"] == "job1", jobs
                assert jobs[0]["outputs"] == rich, jobs[0]["outputs"]

                async with session.get(base + "/api/jobs/job1") as resp:
                    assert resp.status == 200, resp.status
                    one = await resp.json()
                assert one["id"] == "job1" and one["outputs"] == rich
                assert one["bindings"] == {"df": {"summary": "DataFrame"}}, one["bindings"]

                async with session.get(base + "/api/jobs/nope") as resp:
                    assert resp.status == 404, resp.status

                async with session.get(base + "/api/cells") as resp:
                    cells = await resp.json()
                assert cells[0]["id"] == "c1" and cells[0]["outputs"] == rich

                async with session.get(base + "/api/resources") as resp:
                    resources = await resp.json()
                assert resources[0]["id"] == "r1" and resources[0]["html"] == "<b>hi</b>"
        finally:
            await runner.cleanup()

    asyncio.run(main())
    print("api-ok")
  '';
  apiSmoke =
    pkgs.runCommand "ix-mcp-api-smoke"
    {
      nativeBuildInputs = [mcpPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${mcpPython}/bin/python3 ${apiTest} >stdout 2>stderr || {
        echo "ix-mcp api smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'api-ok' stdout || {
        echo "ix-mcp api smoke did not confirm the embedding data API:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
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
        pkgs.nushell
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

  # Issue #1754: per-cell static type checking (ty) before execution, plus the
  # bug 1-3 regressions (await-a-failed-job re-raises; Job/Result accessor
  # symmetry; fsearch partial-on-timeout + limit short-circuit). The type-check
  # tests need ty resolvable and its diagnostics stable, so ty is provided on the
  # env exactly as the wrapper sets it; rg/fd back the fsearch limit assertion.
  # A dedicated interpreter adds pytest (the bare mcpPython omits it).
  typecheckTestPython = mcpPythonInterp.withPackages (ps: (mcpPythonPackages ps) ++ [ps.pytest]);
  typecheckSmoke =
    pkgs.runCommand "ix-mcp-typecheck-smoke"
    {
      nativeBuildInputs = [
        typecheckTestPython
        pkgs.ty
        pkgs.ripgrep
        pkgs.fd
      ];
      strictDeps = true;
      meta.description = "per-cell type check (ty) + issue #1754 bug 1-3 regressions + sh exit surfacing (#1766)";
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      export IX_MCP_TY_BIN=${lib.escapeShellArg tyBin}
      export IX_MCP_TY_PYTHON=${lib.escapeShellArg mcpPython.interpreter}
      # The edited ix_notebook_mcp / fsearch / sh live in the interpreter's
      # site-packages (built from this worktree's source), so the tests import
      # them from there; only the test files are copied in (a bare store path of
      # a single .py is read by pytest as a directory).
      cp ${./tests/test_typecheck.py} test_typecheck.py
      cp ${./tests/test_job_await_errors.py} test_job_await_errors.py
      cp ${./tests/test_fsearch_partial.py} test_fsearch_partial.py
      # sh Output rendering regressions (issue #1766: a failed build must not
      # read as success/still-running); imports the site-packages sh module.
      cp ${./tests/test_sh_module.py} test_sh_module.py
      ${lib.getExe typecheckTestPython} -m pytest \
        test_typecheck.py test_job_await_errors.py test_fsearch_partial.py \
        test_sh_module.py \
        -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp typecheck smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';

  # The session-file contract: run cells against a session store, checkpoint,
  # "restart" into a fresh namespace, and reopen -- the checkpoint restores the
  # state instantly (including a function defined in a cell, which needs the
  # bundled dill), the one cell newer than the checkpoint replays, a row left
  # 'running' by the dead server is marked interrupted, and a second reopen has
  # nothing to replay (the restore folds everything into a fresh checkpoint).
  sessionTestPy = pkgs.writeText "ix-mcp-session-test.py" ''
    # python
    import asyncio
    import tempfile

    import dill  # the checkpoint serializer must be bundled in this interpreter

    from ix_notebook_mcp import runtime, store

    path = tempfile.mktemp(suffix=".ixnb")

    def wire(conn, ns):
        runtime._store = store
        runtime._store_conn = conn
        runtime._user_ns = ns
        runtime._SESSION = True
        runtime._baseline_names = frozenset(ns)

    async def first_run():
        conn = store.connect(path)
        ns = {"Result": runtime.Result}
        wire(conn, ns)
        a = await runtime.__ix_run("x = 40\ndef double(n):\n    return n * 2\nResult.ok('a')")
        assert a.status == "done", (a.status, a.error)
        await runtime._snapshot_now()
        b = await runtime.__ix_run("y = double(x) + 4\nResult.ok('b')")
        assert b.status == "done", (b.status, b.error)
        # A row left 'running' by a server that died mid-cell.
        store.start(conn, id="dead", name="dead", code="zz", started_at=1.0)
        conn.close()

    asyncio.run(first_run())

    async def reopen():
        conn = store.connect(path)
        assert store.mark_interrupted(conn, ended_at=2.0) == 1
        assert store.get(conn, "dead")["status"] == "interrupted"
        ns = {"Result": runtime.Result}
        wire(conn, ns)
        runtime.jobs.clear()
        await runtime.__ix_restore()
        snap = store.latest_snapshot(conn)
        assert snap is not None, "restore must fold a fresh checkpoint"
        assert store.replayable(conn, since=snap["created_at"]) == [], "second reopen must replay nothing"
        conn.close()
        return ns

    ns = asyncio.run(reopen())
    assert ns["x"] == 40, ns.get("x")
    assert ns["double"](3) == 6
    assert ns["y"] == 84, ns.get("y")
    print("session-ok")
  '';
  sessionSmoke =
    pkgs.runCommand "ix-mcp-session-smoke"
    {
      nativeBuildInputs = [mcpPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe mcpPython} ${sessionTestPy} >stdout 2>stderr || {
        echo "ix-mcp session smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'session-ok' stdout || {
        echo "ix-mcp session smoke did not confirm the reopen contract:" >&2
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
    # python
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
            # a wedged reply still carries elapsed_s (the slowest case the field
            # exists to surface), reporting the seconds the call blocked
            assert isinstance(summary["elapsed_s"], float) and summary["elapsed_s"] >= 0.5, summary

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
            from mcp.shared.exceptions import McpError

            set_config(config)
            set_kernel(kernel)

            try:
                await tools.python_exec("Result.ok('blocked')", budget=1.0, intent="blocked first")
            except McpError as exc:
                assert "session_set_name" in str(exc), exc
            else:
                raise AssertionError("python_exec ran before the session was named")

            named = await tools.session_set_name("wedge smoke")
            assert "wedge smoke" in " ".join(getattr(c, "text", "") or "" for c in named), named
            topic = await tools.topic_set("wedge validation")
            assert "wedge validation" in " ".join(getattr(c, "text", "") or "" for c in topic), topic

            started = loop.time()
            clamped = await tools.python_exec(
                "await asyncio.sleep(30)\nResult.ok('done')", budget=600.0, intent="bigbudget"
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
      nativeBuildInputs = [mcpPython];
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
    # python
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

    # The model-facing view (IX_LLM_MIME: the exact llm_result text plus downscaled
    # images) rides into the stored bundle so the dashboard's raw-LLM toggle can
    # show precisely what the agent received, not just the human HTML.
    llm_bundle = runtime._result_bundle(
        runtime.Result(user_html="<b>chart</b>", llm_result="a chart of x", llm_images=[b"\x89PNG\r\n"])
    )
    assert runtime.IX_LLM_MIME in llm_bundle["data"], list(llm_bundle["data"])
    decoded = json.loads(llm_bundle["data"][runtime.IX_LLM_MIME])
    assert decoded["text"] == "a chart of x" and len(decoded["images"]) == 1, decoded
    # A result with no model images still carries IX_LLM_MIME so to_mcp can prefer
    # the explicit model view over any human HTML fallback.
    plain_bundle = runtime._result_bundle(runtime.Result(user_html="<b>hi</b>", llm_result="hi"))
    assert json.loads(plain_bundle["data"][runtime.IX_LLM_MIME]) == {"text": "hi", "images": []}
    # A huge llm_result is clipped to the same cap as any other text mime, so it
    # can never bypass the limit into the store / each dashboard poll.
    big = runtime._result_bundle(
        runtime.Result(user_html="<b>x</b>", llm_result="z" * 500_000, llm_images=[b"\x89PNG\r\n"])
    )
    big_text = json.loads(big["data"][runtime.IX_LLM_MIME])["text"]
    assert big_text.endswith("[truncated]") and len(big_text) <= runtime._MAX_TEXT_BUNDLE + 32, len(big_text)

    # A tuple/list carrying a rich value (a DataFrame) renders each element with
    # its own view, stacked, instead of stringifying the frame into a one-column
    # table -- Result((repr_text, df)) shows the text AND the real table.
    stacked = runtime.Result.of(("GrepResult: 0 matches", pl.DataFrame({"a": [1, 2]})))
    assert stacked.user_html.count("<table") == 1, stacked.user_html[:200]
    assert "GrepResult: 0 matches" in stacked.llm_result and "shape:" in stacked.llm_result, stacked.llm_result
    direct_df = runtime.Result.of(pl.DataFrame({"a": [1, 2], "b": ["x", "y"]}))
    assert '[[a, b]; [1, "x"], [2, "y"]]' in direct_df.llm_result, direct_df.llm_result
    assert "┌" not in direct_df.llm_result, direct_df.llm_result
    # A plain list of scalars is still ONE table (not stacked), unchanged.
    scalars = runtime.Result.of([1, 2, 3])
    assert scalars.user_html.count("<table") == 1, scalars.user_html[:200]
    # Stacking preserves a nested Result's model images (Result.of copies a
    # Result faithfully instead of rebuilding it from its display bundle).
    inner = runtime.Result(user_html="<b>x</b>", llm_result="x", llm_images=[b"\x89PNG\r\n"])
    nested = runtime.Result.of([inner, pl.DataFrame({"a": [1]})])
    assert len(nested.llm_images) == 1, ("nested Result dropped its images", nested.llm_images)

    # The table protocol: a non-DataFrame value exposing _ix_to_frame_() renders
    # as its polars frame: a styled table for the human, compact NUON for the
    # model -- instead of its one-line summary repr, so a rich result type shows
    # the model the real rows, not just a count.
    class _Framed:
        def _ix_to_frame_(self):
            return pl.DataFrame({"path": ["a.py"], "line": [3]})

        def __repr__(self):
            return "Framed: 1 match (summary)"

    framed = runtime.Result.of(_Framed())
    assert "<table" in framed.user_html, framed.user_html[:200]
    assert framed.llm_result.startswith("shape: (1, 2)") and "a.py" in framed.llm_result, framed.llm_result
    assert "summary" not in framed.llm_result, "must render the frame, not the summary repr"

    # Jupyter-style rich hooks can split a bare object's human HTML from its
    # model-facing text without manually constructing Result(...).
    class _Widget:
        def _repr_html_(self):
            return "<strong>human</strong>"

        def _repr_llm_(self):
            return "model-view"

    split = runtime.Result.of(_Widget())
    assert split.user_html == "<strong>human</strong>", split.user_html
    assert split.llm_result == "model-view", split.llm_result
    assert runtime._nuon([{"a": 1, "b": 2}, {"a": 5, "b": 7}]) == "[[a, b]; [1, 2], [5, 7]]"

    # A hook that raises or returns a non-frame is ignored: fall back to the
    # normal repr path rather than blowing up the result.
    class _BadFrame:
        def _ix_to_frame_(self):
            raise RuntimeError("nope")

    assert "BadFrame" in runtime.Result.of(_BadFrame()).llm_result

    # A plain string is rendered as output, not a Python literal: the model gets
    # it verbatim with terminal escapes stripped (no `\n` / `\x1b` repr noise),
    # and the human gets the same text as an HTML <pre>, escaped, with no raw
    # control bytes. This is the read-tool treatment for a streamed Result.
    s = runtime.Result.of("line1\nline2\n\x1b[0;32mgreen\x1b[0m")
    assert s.llm_result == "line1\nline2\ngreen", repr(s.llm_result)
    assert "\x1b" not in s.user_html and s.user_html.startswith("<pre"), s.user_html[:80]
    # A short string carries no surrounding repr quotes.
    assert runtime.Result.of("hello").llm_result == "hello"
    # HTML metacharacters are escaped for the human, verbatim for the model.
    esc = runtime.Result.of("a <b> & c")
    assert esc.llm_result == "a <b> & c" and "&lt;b&gt;" in esc.user_html, esc.user_html
    # An explicit llm_result still overrides the verbatim model text.
    assert runtime.Result.of("raw", llm_result="override").llm_result == "override"
    # The shared ANSI stripper lives in the runtime (the bundled `sh` helper
    # imports it rather than keeping a second copy).
    assert runtime._strip_ansi("\x1b[31mx\x1b[0m") == "x"


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
        # frame (NUON rows 1/2/3), not collapsed to just the first value.
        assert "true" in multi_text and "[[value]; [1], [2], [3]]" in multi_text, ("multi-value dropped a value", multi_text)


    asyncio.run(main())
    print("rich-ok")
  '';
  # Proves the yielding-cell behavior end to end: a cell that `yield`s streams
  # every yielded value to the store (the dashboard) and to the model (to_mcp),
  # keeps its top-level names in the namespace like a normal cell, and a
  # non-Result yield renders through Result.of. A plain (non-yielding) cell is
  # unchanged. In process (a shell, the store), no kernel boot or network, so
  # the sandbox runs it.
  yieldTestPy = pkgs.writeText "ix-mcp-yield-test.py" ''
    # python
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

        # A non-Result yield streams too: any value renders through Result.of,
        # exactly like a trailing expression.
        bare = await run("yield 123", budget=3.0, name="bare")
        await bare.task
        assert bare.status == "done", (bare.status, bare.error)
        bare_outs = json.loads(
            conn.execute("SELECT outputs FROM executions WHERE id = ?", (bare.id,)).fetchone()[0]
        )
        bare_mcp = outputs.to_mcp(
            [{"output_type": "display_data", "data": o["data"], "metadata": {}} for o in bare_outs]
        )
        bare_texts = [c.text for c in bare_mcp if getattr(c, "text", None) is not None]
        assert any("123" in t for t in bare_texts), bare_texts

        # A normal (non-yielding) cell is unchanged.
        plain = await run("Result.ok('plain')", budget=3.0, name="plain")
        await plain.task
        assert plain.status == "done", (plain.status, plain.error)


    asyncio.run(main())
    print("yield-ok")
  '';

  yieldSmoke =
    pkgs.runCommand "ix-mcp-yield-smoke"
    {
      nativeBuildInputs = [mcpPython];
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
      nativeBuildInputs = [mcpPython];
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
    # python
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
      nativeBuildInputs = [mcpPython];
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
    # python
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
      nativeBuildInputs = [mcpPython];
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

  # The imessage module: read the Messages/Contacts SQLite databases into polars
  # and (without sending) validate the AppleScript send path. Hermetic -- it
  # builds tiny fixture databases with the real schema subset and round-trips an
  # archived NSAttributedString through Foundation, so the sandbox runs it.
  imessageTestPy = pkgs.writeText "ix-mcp-imessage-test.py" ''
    # python

    import os
    import sqlite3
    import tempfile
    from datetime import datetime, timezone

    import polars as pl

    import imessage

    assert imessage.CHAT_DB.endswith("Library/Messages/chat.db"), imessage.CHAT_DB

    # attributedBody round-trip: archive an NSAttributedString the way macOS does,
    # then assert our NSUnarchiver-based decoder recovers the exact text (incl.
    # non-ASCII), and that junk/None decode to None rather than raising.
    import Foundation

    s = Foundation.NSAttributedString.alloc().initWithString_("héllo 👋 world")
    blob = bytes(Foundation.NSArchiver.archivedDataWithRootObject_(s))
    assert imessage._decode_attributed_body(blob) == "héllo 👋 world"
    assert imessage._decode_attributed_body(b"not an archive") is None
    assert imessage._decode_attributed_body(None) is None

    # normalization lines up handles with address-book entries.
    assert imessage._norm("+1 (202) 555-0123") == imessage._norm("2025550123")
    assert imessage._norm("Me@Example.COM ") == "me@example.com"

    # reply/tapback reference parsing: strip the "p:<part>/" or "bp:" prefix to the
    # bare GUID, and decode associated_message_type into a tapback label.
    assert imessage._bare_guid("p:0/ABC") == "ABC"
    assert imessage._bare_guid("bp:XYZ") == "XYZ"
    assert imessage._bare_guid("ABC") == "ABC"
    assert imessage._bare_guid(None) is None
    assert imessage._tapback(2000) == "loved"
    assert imessage._tapback(2005) == "questioned"
    assert imessage._tapback(3003) == "removed-laughed"
    # base outside 2xxx/3xxx is not a tapback even when the low digit collides
    # with a reaction offset (1000 is an inline association, not a "loved").
    assert imessage._tapback(1000) is None
    assert imessage._tapback(2) is None
    assert imessage._tapback(0) is None
    assert imessage._tapback(None) is None

    work = tempfile.mkdtemp()
    chat = os.path.join(work, "chat.db")
    con = sqlite3.connect(chat)
    con.executescript(
        """
        CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
        CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, display_name TEXT, chat_identifier TEXT, service_name TEXT);
        CREATE TABLE message (ROWID INTEGER PRIMARY KEY, guid TEXT, date INTEGER, text TEXT, attributedBody BLOB, is_from_me INTEGER, is_read INTEGER, service TEXT, handle_id INTEGER, thread_originator_guid TEXT, associated_message_guid TEXT, associated_message_type INTEGER DEFAULT 0, date_edited INTEGER DEFAULT 0, date_retracted INTEGER DEFAULT 0);
        CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
        CREATE TABLE message_attachment_join (message_id INTEGER, attachment_id INTEGER);
        """
    )
    apple_epoch = datetime(2001, 1, 1, tzinfo=timezone.utc)

    def ns(dt):
        return int((dt - apple_epoch).total_seconds() * 1_000_000_000)

    t1 = datetime(2024, 1, 2, 3, 4, 5, tzinfo=timezone.utc)
    t2 = datetime(2024, 1, 2, 3, 5, 5, tzinfo=timezone.utc)
    con.execute("INSERT INTO handle VALUES (1, '+12025550123')")
    con.execute("INSERT INTO chat VALUES (1, NULL, '+12025550123', 'iMessage')")
    con.execute("INSERT INTO message (ROWID, guid, date, text, attributedBody, is_from_me, is_read, service, handle_id) VALUES (1, 'm1', ?, 'plain text', NULL, 1, 1, 'iMessage', 1)", (ns(t1),))
    con.execute("INSERT INTO message (ROWID, guid, date, text, attributedBody, is_from_me, is_read, service, handle_id) VALUES (2, 'm2', ?, NULL, ?, 0, 0, 'iMessage', 1)", (ns(t2), blob))
    con.execute("INSERT INTO chat_message_join VALUES (1, 1), (1, 2)")
    con.execute("INSERT INTO message_attachment_join VALUES (2, 99)")
    con.commit()
    con.close()

    # resolve_names defaults True; with no address book under $HOME it must degrade
    # to name=None rather than fail.
    df = imessage.messages(db=chat, limit=10)
    assert df.height == 2, df
    assert df.schema["date"] == pl.Datetime("ns", "UTC"), df.schema
    # newest first; attributedBody is decoded into `text`.
    assert df["rowid"].to_list() == [2, 1], df
    assert df["text"].to_list() == ["héllo 👋 world", "plain text"], df["text"].to_list()
    assert df["is_from_me"].to_list() == [False, True], df
    assert df["name"].to_list() == [None, None], df
    # the relational columns default cleanly when a message is none of these.
    assert df["reply_to_guid"].to_list() == [None, None], df
    assert df["reply_to_rowid"].to_list() == [None, None], df
    assert df["tapback"].to_list() == [None, None], df
    assert df["edited"].to_list() == [False, False], df
    assert df["unsent"].to_list() == [False, False], df

    row2 = df.filter(pl.col("rowid") == 2)
    assert row2["date"][0] == t2, (row2["date"][0], t2)
    assert row2["n_attachments"][0] == 1, row2

    # filters: from_me, contact (by handle, normalized), since, and a no-match.
    assert imessage.messages(db=chat, from_me=True).height == 1
    assert imessage.messages(db=chat, contact="(202) 555-0123").height == 2
    assert imessage.messages(db=chat, contact="+19999999999").height == 0
    since = imessage.messages(db=chat, since=t2)
    assert since.height == 1 and since["rowid"][0] == 2, since
    # a no-match still returns the full, typed (datetime) schema.
    assert imessage.messages(db=chat, contact="+19999999999").schema["date"] == pl.Datetime("ns", "UTC")

    chats = imessage.chats(db=chat)
    assert chats.height == 1 and chats["n_messages"][0] == 2, chats
    assert chats["chat_identifier"][0] == "+12025550123", chats
    assert chats.schema["last_date"] == pl.Datetime("ns", "UTC"), chats.schema

    # Reads must see un-checkpointed WAL rows: chat.db runs in WAL mode and a
    # message you just sent sits in the -wal until a checkpoint. A writer keeps a
    # WAL connection open (no checkpoint) after inserting; messages() must still
    # see the new row -- it would not under immutable=1. Guards send-then-read-back.
    writer = sqlite3.connect(chat)
    writer.execute("PRAGMA journal_mode=WAL")
    writer.execute(
        "INSERT INTO message (ROWID, guid, date, text, attributedBody, is_from_me, is_read, service, handle_id) VALUES (3, 'm3', ?, 'in the wal', NULL, 1, 1, 'iMessage', 1)",
        (ns(datetime(2024, 1, 2, 3, 6, 5, tzinfo=timezone.utc)),),
    )
    writer.execute("INSERT INTO chat_message_join VALUES (1, 3)")
    writer.commit()
    assert "in the wal" in imessage.messages(db=chat)["text"].to_list(), "WAL rows must be visible"
    writer.close()

    # reply threading, tapbacks, edits, and unsends: insert one of each and assert
    # messages() resolves a threaded reply to its originator and decodes the rest.
    con = sqlite3.connect(chat)
    cols2 = "(ROWID, guid, date, text, attributedBody, is_from_me, is_read, service, handle_id, thread_originator_guid, associated_message_guid, associated_message_type, date_edited, date_retracted)"
    t3 = datetime(2024, 1, 2, 4, 0, 0, tzinfo=timezone.utc)
    # a threaded reply to message 1 ("plain text"); the originator ref carries a
    # "p:0/" part prefix that must be stripped back to the bare guid "m1".
    con.execute("INSERT INTO message " + cols2 + " VALUES (10, 'm10', ?, 'a reply', NULL, 0, 1, 'iMessage', 1, 'p:0/m1', NULL, 0, 0, 0)", (ns(t3),))
    # a Loved tapback on message 2, and a removed-liked on message 1.
    con.execute("INSERT INTO message " + cols2 + " VALUES (11, 'm11', ?, 'Loved a message', NULL, 1, 1, 'iMessage', 1, NULL, 'p:0/m2', 2000, 0, 0)", (ns(t3),))
    con.execute("INSERT INTO message " + cols2 + " VALUES (12, 'm12', ?, 'Removed a like', NULL, 1, 1, 'iMessage', 1, NULL, 'p:0/m1', 3001, 0, 0)", (ns(t3),))
    # an edited message and an unsent (retracted) message.
    con.execute("INSERT INTO message " + cols2 + " VALUES (13, 'm13', ?, 'fixed typo', NULL, 1, 1, 'iMessage', 1, NULL, NULL, 0, ?, 0)", (ns(t3), ns(t3)))
    con.execute("INSERT INTO message " + cols2 + " VALUES (14, 'm14', ?, 'oops', NULL, 1, 1, 'iMessage', 1, NULL, NULL, 0, 0, ?)", (ns(t3), ns(t3)))
    # associated_message_type 1000 is a non-tapback association: it must not be
    # mislabeled "loved" just because its low digit is the loved offset.
    con.execute("INSERT INTO message " + cols2 + " VALUES (15, 'm15', ?, 'inline assoc', NULL, 1, 1, 'iMessage', 1, NULL, 'p:0/m1', 1000, 0, 0)", (ns(t3),))
    con.execute("INSERT INTO chat_message_join VALUES (1, 10), (1, 11), (1, 12), (1, 13), (1, 14), (1, 15)")
    con.commit()
    con.close()

    thr = imessage.messages(db=chat, limit=50)
    reply = thr.filter(pl.col("rowid") == 10).row(0, named=True)
    assert reply["reply_to_guid"] == "m1", reply
    assert reply["reply_to_rowid"] == 1, reply
    assert reply["reply_to_text"] == "plain text", reply
    love = thr.filter(pl.col("rowid") == 11).row(0, named=True)
    assert love["tapback"] == "loved" and love["tapback_target_guid"] == "m2", love
    removed = thr.filter(pl.col("rowid") == 12).row(0, named=True)
    assert removed["tapback"] == "removed-liked" and removed["tapback_target_guid"] == "m1", removed
    assert thr.filter(pl.col("rowid") == 13)["edited"][0] is True
    assert thr.filter(pl.col("rowid") == 14)["unsent"][0] is True
    assert thr.filter(pl.col("rowid") == 15)["tapback"][0] is None, thr.filter(pl.col("rowid") == 15)
    # a plain message carries none of this metadata.
    plain = thr.filter(pl.col("rowid") == 1).row(0, named=True)
    assert plain["reply_to_guid"] is None and plain["tapback"] is None, plain
    assert plain["edited"] is False and plain["unsent"] is False, plain

    # legacy chat.db compatibility: a pre-macOS-13 schema has no
    # thread_originator_guid / date_edited / date_retracted columns. messages()
    # must select those only when present and degrade to empty fields, not raise.
    legacy = os.path.join(work, "legacy.db")
    con = sqlite3.connect(legacy)
    con.executescript(
        """
        CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
        CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, display_name TEXT, chat_identifier TEXT, service_name TEXT);
        CREATE TABLE message (ROWID INTEGER PRIMARY KEY, guid TEXT, date INTEGER, text TEXT, attributedBody BLOB, is_from_me INTEGER, is_read INTEGER, service TEXT, handle_id INTEGER);
        CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
        CREATE TABLE message_attachment_join (message_id INTEGER, attachment_id INTEGER);
        """
    )
    con.execute("INSERT INTO handle VALUES (1, '+12025550123')")
    con.execute("INSERT INTO message (ROWID, guid, date, text, is_from_me, is_read, service, handle_id) VALUES (1, 'g1', ?, 'legacy', 1, 1, 'iMessage', 1)", (ns(t1),))
    con.execute("INSERT INTO chat_message_join VALUES (1, 1)")
    con.commit()
    con.close()
    leg = imessage.messages(db=legacy, limit=5)
    assert leg.height == 1 and leg["text"][0] == "legacy", leg
    assert leg["reply_to_guid"][0] is None and leg["tapback"][0] is None, leg
    assert leg["edited"][0] is False and leg["unsent"][0] is False, leg

    # contacts: phones/emails aggregate into list columns under one display name.
    ab = os.path.join(work, "ab.abcddb")
    con = sqlite3.connect(ab)
    con.executescript(
        """
        CREATE TABLE ZABCDRECORD (Z_PK INTEGER PRIMARY KEY, ZFIRSTNAME TEXT, ZLASTNAME TEXT, ZORGANIZATION TEXT, ZNICKNAME TEXT);
        CREATE TABLE ZABCDPHONENUMBER (Z_PK INTEGER PRIMARY KEY, ZOWNER INTEGER, ZFULLNUMBER TEXT);
        CREATE TABLE ZABCDEMAILADDRESS (Z_PK INTEGER PRIMARY KEY, ZOWNER INTEGER, ZADDRESS TEXT);
        """
    )
    con.execute("INSERT INTO ZABCDRECORD VALUES (1, 'Ada', 'Lovelace', NULL, NULL)")
    con.execute("INSERT INTO ZABCDPHONENUMBER VALUES (1, 1, '+1 (202) 555-0123')")
    con.execute("INSERT INTO ZABCDPHONENUMBER VALUES (2, 1, '+12025550124')")
    con.execute("INSERT INTO ZABCDEMAILADDRESS VALUES (1, 1, 'ada@x.com')")
    con.commit()
    con.close()
    co = imessage.contacts(db=ab)
    assert co.height == 1, co
    rec = co.row(0, named=True)
    assert rec["name"] == "Ada Lovelace", rec
    assert set(rec["phones"]) == {"+1 (202) 555-0123", "+12025550124"}, rec
    assert rec["emails"] == ["ada@x.com"], rec

    # send: rejects an unknown service and never sends here; the script carries the
    # service token and takes recipient/body as run-arguments (no injection).
    try:
        imessage.send("+12025550123", "nope", service="bogus")
    except ValueError:
        pass
    else:
        raise SystemExit("send must reject an unknown service")
    assert "service type = iMessage" in imessage._SEND_SCRIPT.format(service="iMessage")

    print("imessage-ok")
  '';
  imessageSmoke =
    pkgs.runCommand "ix-mcp-imessage-smoke"
    {
      nativeBuildInputs = [mcpPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe mcpPython} ${imessageTestPy} >stdout 2>stderr || {
        echo "ix-mcp imessage smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'imessage-ok' stdout || {
        echo "ix-mcp imessage smoke did not confirm the imessage module:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';

  # The maps module: pure-helper checks that need no network or location
  # permission (the nix sandbox has neither). Exercises the radius->region span
  # math (incl. the latitude cosine correction) and the polars schema shapes, and
  # confirms the public coroutines and MapKit binding are present.
  mapsTestPy = pkgs.writeText "ix-mcp-maps-test.py" ''
    # python
    import inspect
    import math

    import polars as pl

    import maps
    import MapKit

    # Public coroutine surface is callable and async.
    for name in ("nearby", "geocode", "reverse_geocode"):
        fn = getattr(maps, name)
        assert inspect.iscoroutinefunction(fn), name

    # MapKit binding loads (the place-search class is present).
    assert callable(MapKit.MKLocalSearch.alloc), "MKLocalSearch missing"

    # region(): span is the full width/height, so twice the radius in degrees;
    # latitude degrees are constant, longitude degrees shrink with cos(latitude).
    (clat, clng), (lat_delta, lng_delta) = maps._region(0.0, 0.0, 1000.0)
    assert (clat, clng) == (0.0, 0.0)
    assert math.isclose(lat_delta, 2000.0 / 111320.0, rel_tol=1e-9), lat_delta
    assert math.isclose(lng_delta, lat_delta, rel_tol=1e-9), (lat_delta, lng_delta)
    # At 60 deg latitude cos=0.5, so longitude span is ~2x the latitude span.
    (_c2, (lat60, lng60)) = maps._region(60.0, 0.0, 1000.0)
    assert math.isclose(lng60 / lat60, 2.0, rel_tol=1e-6), (lat60, lng60)

    # Schemas: nearby is the placemark schema plus the POI columns.
    placemark = set(maps._placemark_schema(pl))
    nearby = set(maps._nearby_schema(pl))
    assert {"name", "latitude", "longitude", "country"} <= placemark, placemark
    assert nearby - placemark == {"category", "phone"}, nearby - placemark

    print("maps-ok")
  '';
  mapsSmoke =
    pkgs.runCommand "ix-mcp-maps-smoke"
    {
      nativeBuildInputs = [mcpPython];
      strictDeps = true;
    }
    ''
      ${lib.getExe mcpPython} ${mapsTestPy} >stdout 2>stderr || {
        echo "ix-mcp maps smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'maps-ok' stdout || {
        echo "ix-mcp maps smoke did not confirm the maps module:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';

  # The ghostty module: pure-logic checks that need no Ghostty, osascript, or
  # display (the nix sandbox has none). Exercises the AppleScript-escape guard
  # (the injection fix), the ps-ancestry parse/walk, and the surfaces readout
  # split, and confirms the public coroutine surface.
  ghosttyTestPy = pkgs.writeText "ix-mcp-ghostty-test.py" ''
    # python
    import inspect

    import ghostty

    # Public surface is callable and async.
    for name in (
        "surfaces", "my_tty", "my_surface", "close", "close_me",
        "focus", "activate", "is_running",
    ):
        assert inspect.iscoroutinefunction(getattr(ghostty, name)), name

    # _escape_applescript neutralises a quote-injection payload, rejects newlines.
    assert ghostty._escape_applescript('a"b') == 'a\\"b'
    assert ghostty._escape_applescript("c\\d") == "c\\\\d"
    try:
        ghostty._escape_applescript("a\nb")
    except ghostty.GhosttyError:
        pass
    else:
        raise SystemExit("escape did not reject a newline")

    # _selector escapes the value: no raw quote reaches the whose-clause, so the
    # `" or true or "` predicate-injection cannot match every surface.
    sel = ghostty._selector(tty=None, id='" or true or "')
    assert sel.count('\\"') == 2, sel
    assert ghostty._selector(tty="/dev/ttys001", id=None) == (
        '(first terminal whose tty is "/dev/ttys001")'
    )
    # Fails closed unless exactly one selector is given (both set or both unset).
    for bad in ({}, {"tty": "/dev/ttys001", "id": "X"}):
        try:
            ghostty._selector(tty=bad.get("tty"), id=bad.get("id"))
        except ghostty.GhosttyError:
            pass
        else:
            raise SystemExit(f"_selector accepted ambiguous args: {bad}")

    # _parse_ps + _walk_to_tty resolve the controlling tty from a synthetic table.
    tree = ghostty._parse_ps("100 1 ??\n200 100 ttys003\n300 200 ??\n")
    assert tree[200] == (100, "ttys003"), tree
    assert ghostty._walk_to_tty(tree, 300) == "/dev/ttys003"
    assert ghostty._walk_to_tty(tree, 100) is None

    # _parse_surfaces splits the RS/FS readout and coerces pid to int.
    raw = "ID1\x1f/dev/ttys001\x1f42\x1f/tmp\x1fTitle\x1e"
    assert ghostty._parse_surfaces(raw) == [
        {"id": "ID1", "tty": "/dev/ttys001", "pid": 42,
         "working_directory": "/tmp", "name": "Title"}
    ]
    assert set(ghostty._surface_schema()) == set(ghostty._FIELDS)

    print("ghostty-ok")
  '';
  ghosttySmoke =
    pkgs.runCommand "ix-mcp-ghostty-smoke"
    {
      nativeBuildInputs = [mcpPython];
      strictDeps = true;
    }
    ''
      ${lib.getExe mcpPython} ${ghosttyTestPy} >stdout 2>stderr || {
        echo "ix-mcp ghostty smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'ghostty-ok' stdout || {
        echo "ix-mcp ghostty smoke did not confirm the ghostty module:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';

  # The view module: tabular helpers return plain polars DataFrames (so they stay
  # composable), the file helpers return a Code view whose repr is the raw text,
  # and df_html renders the styled table the kernel installs globally. Pure local
  # FS over the bundled view/polars/pygments, so the sandbox runs it.
  viewTestPy = pkgs.writeText "ix-mcp-view-test.py" ''
    # python
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
    # Content/file search is no longer in `view`: it lives in the top-level
    # `grep`/`find` builtins (rg/fd-backed), exercised by the fsearch check.

    tr = view.tree(base, depth=1)
    assert isinstance(tr, pl.DataFrame) and "depth" in tr.columns

    out = view.df_html(lsdf)
    assert "<table" in out and "rows" in out and "tabular-nums" in out, out[:120]
    # The modern grid ships a client-side filter box, a sortable (clickable,
    # aria-sort) sticky header, and dtype-classed cells -- inline JS/CSS, no CDN.
    assert 'input class="q"' in out and "aria-sort" in out and "sticky" in out, out[:200]
    # Coloring lives in the ONE shared stylesheet keyed by dtype class, not a
    # per-cell style= attribute -- that keeps a wide frame's body small enough
    # for the dashboard's Loro pane diff. A 40x40 int frame must stay well under
    # the ~200KB range that wedged the aggregator, and far below the old build
    # (which repeated a full inline style on every cell).
    wide = pl.DataFrame({f"c{j}": range(40) for j in range(40)})
    wout = view.df_html(wide)
    assert 'style="color:' not in wout, "cells must be class-styled, not inline"
    assert len(wout) < 130_000, len(wout)

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
      nativeBuildInputs = [mcpPython];
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
    # python
    import asyncio

    import sh
    from ix_notebook_mcp.runtime import Result


    async def main():
        # A command that emits an SGR color escape around its output.
        colored = await sh._exec(r"printf '\033[31mred\033[0m\n'", cwd=".")
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
        # The ANSI helpers are the runtime's single implementation, imported
        # here rather than duplicated in the sh module.
        from ix_notebook_mcp import runtime as _rt

        assert sh._strip_ansi is _rt._strip_ansi and sh._ansi_to_html is _rt._ansi_to_html

        # argv form, and a non-zero exit is surfaced (not swallowed): typed
        # (.code with the .exit_code/.returncode aliases), falsy, and loud at
        # BOTH ends of the model view so a head-read of a long log sees the
        # failure as surely as a tail-read (issue #1766).
        failed = await sh._exec(["false"], cwd=".")
        assert not failed.ok and failed.code == 1, failed.code
        assert failed.exit_code == 1 and failed.returncode == 1, failed.exit_code
        assert bool(failed) is False, "a failed Output must be falsy"
        assert "[exit 1]" in failed.llm_result, failed.llm_result
        assert failed.llm_result.splitlines()[0].startswith("[exit 1]"), failed.llm_result
        # ...and even an output-less failure both leads and TRAILS with the
        # marker, so a tail-read never lands on command text.
        assert failed.llm_result.rstrip().endswith("\n[exit 1]"), failed.llm_result
        noisy = await sh._exec("echo diagnostic-text; exit 3", cwd=".")
        first, *rest = noisy.llm_result.splitlines()
        assert first.startswith("[exit 3]") and "exit 3" in first, noisy.llm_result
        assert noisy.llm_result.rstrip().endswith("[exit 3]"), noisy.llm_result
        # ...while .text stays the command's own output, marker-free, so
        # reading diagnostics off a failure is unchanged.
        assert noisy.text.strip() == "diagnostic-text", repr(noisy.text)
        assert "diagnostic-text" in "\n".join(rest), noisy.llm_result

        # Rendered command text is secret-redacted (#1769 post-merge P1): a
        # failing command whose STRING carries a credential shape must not leak
        # it into the model view, the ShellError message, or the dashboard
        # HTML; the raw command stays on .cmd. Fixture token is repeated
        # filler, not a real credential.
        tok = "tok9" * 10
        leak = await sh._exec(f"false Bearer {tok}", cwd=".")
        assert not leak.ok, leak.code
        assert tok not in leak.llm_result, leak.llm_result
        assert "[redacted:bearer_token]" in leak.llm_result.splitlines()[0], leak.llm_result
        assert leak.llm_result.splitlines()[0].split(": ", 1)[1].startswith("false"), (
            leak.llm_result)  # argv[0] survives redaction: still identifiable
        assert tok not in leak._repr_html_() and "[redacted:" in leak._repr_html_()
        assert tok in leak.cmd  # programmatic surface stays raw
        try:
            await sh._exec(f"false token={tok}", check=True, cwd=".")
        except sh.ShellError as exc:
            assert tok not in str(exc), str(exc)
            assert "token=[redacted:credential]" in str(exc), str(exc)
        else:
            raise SystemExit("expected ShellError from check=True")
        # A multi-line command collapses to ONE failure line (tail-reads land
        # on markers, not command fragments).
        multi = await sh._exec("false a \\\n  b", cwd=".")
        assert multi.llm_result.splitlines()[0].startswith("[exit 1]"), multi.llm_result
        assert multi.llm_result.rstrip().endswith("[exit 1]"), multi.llm_result

        # The expected-nonzero class (grep exiting 1 on no match) stays
        # workable: branch on .ok/.code and read .text, nothing raises.
        nomatch = await sh._exec("grep zzz-no-such /dev/null", cwd=".")
        assert not nomatch.ok and nomatch.code == 1, nomatch.code
        assert "[exit" not in nomatch.text, repr(nomatch.text)
        # grep also carries a structured-owner hint; it rides INSIDE the
        # failure markers, so the model text still ends with [exit N].
        assert "[hint:" in nomatch.llm_result, nomatch.llm_result
        assert nomatch.llm_result.rstrip().endswith("[exit 1]"), nomatch.llm_result

        # check=True turns a non-zero exit into a typed error carrying the output.
        try:
            await sh._exec("exit 3", check=True, cwd=".")
        except sh.ShellError as exc:
            assert exc.output.code == 3, exc.output.code
        else:
            raise SystemExit("expected ShellError on a non-zero exit with check=True")

        # An OSC-8 hyperlink (what gh/eza emit under FORCE_COLOR) is a non-CSI
        # escape: the stripper must remove its \x1b bytes too, not just SGR color.
        osc = await sh._exec(r"printf '\033]8;;https://x\033\\link\033]8;;\033\\\n'", cwd=".")
        assert "\x1b" not in osc.text and "link" in osc.text, repr(osc.text)
        assert "\x1b" not in osc.llm_result, repr(osc.llm_result)

        # A timeout must terminate the command's whole group and return promptly,
        # even when the command backgrounds a child that holds the stdout pipe
        # (the case where a naive kill + reap hangs forever).
        loop = asyncio.get_running_loop()
        start = loop.time()
        try:
            await sh._exec("sleep 30 & echo started; wait", timeout=0.5, cwd=".")
        except TimeoutError:
            pass
        else:
            raise SystemExit("expected TimeoutError from a command that outlives its timeout")
        elapsed = loop.time() - start
        assert elapsed < 10, f"timeout did not return promptly: {elapsed:.1f}s"

        # The PUBLIC entry points are retired: `sh()`, `sh.sh()`, `sh.zsh()`,
        # and calling the module all raise a migration hint pointing at
        # `await nu(...)`, so a stale transcript fails loudly rather than
        # shelling out. The private `_exec` (exercised above) is what remains.
        for call in (lambda: sh("printf hi"), lambda: sh.sh("printf hi"), lambda: sh.zsh("print hi")):
            try:
                await call()
            except RuntimeError as exc:
                assert "await nu" in str(exc), exc
            else:
                raise SystemExit("expected a disabled sh()/zsh() to raise a migration hint")

        # A direct runner handle for the composition/streaming checks below.
        direct = await sh._exec("printf hi", cwd=".")
        assert direct.ok and direct.text == "hi", repr(direct.text)

        # cwd defaults to the current directory: no required-kwarg TypeError.
        import os
        here = await sh._exec("pwd")
        assert here.ok and here.text.strip() == os.path.realpath(os.getcwd()), (
            here.text, os.getcwd())

        # An Output composes like its text: slice, concat, contains, len, str.
        assert direct[-1:] == "i" and direct[0] == "h", (direct[-1:], direct[0])
        assert direct + "!" == "hi!" and "say " + direct == "say hi"
        assert "hi" in direct and len(direct) == 2 and str(direct) == "hi"
        # Truthiness is success: empty-but-successful stays truthy (test
        # emptiness with len), and a failed Output is falsy (asserted above).
        assert bool(await sh._exec("true")) is True

        # Output streams to sys.stdout as it arrives (echo=True forces it outside
        # a kernel job), escape-stripped -- so a long command's log lands in the
        # job's pageable stdout even if the cell backgrounds before binding it.
        import contextlib
        import io
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            echoed = await sh._exec(r"printf '\033[31mstreamed\033[0m\n'", echo=True)
        assert "streamed" in buf.getvalue() and "\x1b" not in buf.getvalue(), repr(buf.getvalue())
        assert "streamed" in echoed.text, repr(echoed.text)
        # And echo stays off by default outside a kernel job.
        quiet = io.StringIO()
        with contextlib.redirect_stdout(quiet):
            await sh._exec("printf silent")
        assert quiet.getvalue() == "", repr(quiet.getvalue())
        # A failing command's stream also carries the failure line, so a watcher
        # paging a backgrounded job's stdout (jobs['<id>'].tail()) sees the
        # terminal state even if the Output is never bound (issue #1766).
        fbuf = io.StringIO()
        with contextlib.redirect_stdout(fbuf):
            await sh._exec("echo dying; exit 5", echo=True)
        assert "dying" in fbuf.getvalue(), repr(fbuf.getvalue())
        assert "[exit 5]" in fbuf.getvalue(), repr(fbuf.getvalue())

        # Cancelling the awaiting task kills the child's whole process group:
        # no orphan keeps running (or holding a lock) after a .cancel().
        import signal
        import tempfile
        pidfile = tempfile.mktemp()
        task = asyncio.ensure_future(sh._exec(f"echo $$ > {pidfile}; sleep 30", cwd="."))
        for _ in range(100):
            await asyncio.sleep(0.05)
            try:
                pid = int(open(pidfile).read().strip())
                break
            except (FileNotFoundError, ValueError):
                continue
        else:
            raise SystemExit("child never wrote its pidfile")
        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass
        await asyncio.sleep(0.3)
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            pass  # the group is dead, as required
        else:
            os.kill(pid, signal.SIGKILL)
            raise SystemExit(f"cancel orphaned the child (pid {pid} still alive)")

        # Structured stdout decodes straight to Python (the polars on-ramp).
        doc = await sh._exec("printf '%s' '{\"a\": 1, \"b\": [2, 3]}'", cwd=".")
        assert doc.json() == {"a": 1, "b": [2, 3]}, doc.json()
        rows = await sh._exec("printf '%s\\n%s\\n' '{\"n\": 1}' '{\"n\": 2}'", cwd=".")
        assert rows.jsonl() == [{"n": 1}, {"n": 2}], rows.jsonl()
        # A failed command raises ShellError from json(), never a decode error.
        try:
            (await sh._exec("echo nope; exit 4", cwd=".")).json()
        except sh.ShellError as exc:
            assert exc.output.code == 4, exc.output.code
        else:
            raise SystemExit("expected ShellError from json() on a non-zero exit")

        print("sh-ok", sh.__version__)


    asyncio.run(main())
  '';
  shSmoke =
    pkgs.runCommand "ix-mcp-sh-smoke"
    {
      nativeBuildInputs = [
        mcpPython
        pkgs.zsh
      ];
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
    # python
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
      nativeBuildInputs = [mcpPython];
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

  # The cluster surface (discovery merge, Ray submit return-shape, /api/exec
  # auth) with the two discovery sources, the Ray remote, and the kernel all
  # stubbed -- no live cluster or network. mcpPython carries both `fleet` and
  # `ix_notebook_mcp`, so the script imports them with no PYTHONPATH.
  fleetClusterSmoke =
    pkgs.runCommand "ix-mcp-fleet-cluster-smoke"
    {
      nativeBuildInputs = [mcpPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe mcpPython} ${./tests/fleet_cluster_check.py} >stdout 2>stderr || {
        echo "ix-mcp fleet cluster smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -q '^fleet-cluster-ok' stdout || {
        echo "ix-mcp fleet cluster smoke did not confirm the cluster surface:" >&2
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
    # python
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
      nativeBuildInputs = [mcpPython];
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
    # python
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

  # The browser module: it drives a Chromium-family browser over CDP with the
  # bundled playwright. A real browser needs a display, and we NEVER run headless,
  # so the sandbox cannot launch one; instead this asserts the contract that does
  # not need a browser -- the API shape, the standard/persistent defaults, the
  # never-headless launch argv, the clear error when nothing is on the port, and
  # that api() now lists both the module and the bundled playwright library.
  browserTestPy = pkgs.writeText "ix-mcp-browser-test.py" ''
    # python
    import asyncio
    import sys

    import browser
    from ix_notebook_mcp import runtime

    # Standard CDP port + a persistent, module-owned profile, so repeat launches
    # reuse one instance instead of spawning a new window each time.
    assert browser.DEFAULT_ENDPOINT == "http://127.0.0.1:9222", browser.DEFAULT_ENDPOINT
    assert browser.DEFAULT_APP == "Google Chrome", browser.DEFAULT_APP
    for fn in ("get_or_create_browser", "connect", "context", "page", "goto", "shot", "read", "vdom", "close"):
        assert callable(getattr(browser, fn)), fn
    # `vdom()` returns a Vdom: a clean, filtered, machine-readable map of the page.
    assert isinstance(browser.Vdom, type), browser.Vdom

    udd = browser._default_user_data_dir(browser.DEFAULT_APP)
    assert udd.endswith(".cdp-google-chrome-profile"), udd
    argv = browser._launch_argv(browser.DEFAULT_APP, 9222, udd)
    # The launched browser is ALWAYS a visible window -- never headless.
    assert not any("headless" in a for a in argv), ("launch must never be headless", argv)
    assert "--remote-debugging-port=9222" in argv, argv
    assert ("--user-data-dir=" + udd) in argv, argv
    if sys.platform == "darwin":
        assert argv[:3] == ["open", "-na", "Google Chrome"], argv
    assert browser._port_of("http://127.0.0.1:9222") == 9222

    async def _dead():
        # Nothing is listening on port 1: connect() must fail clearly and point at
        # get_or_create_browser() (which would launch one) rather than hang.
        try:
            await browser.connect("http://127.0.0.1:1")
        except ConnectionError as exc:
            assert "get_or_create_browser" in str(exc), exc
        else:
            raise SystemExit("connect() to a dead port should raise ConnectionError")

    asyncio.run(_dead())

    # Discoverability: api() lists the browser module AND playwright as a bundled
    # library, so neither looks absent to an agent treating api() as the catalog.
    rows = runtime._api_rows()
    wheres = {r["where"] for r in rows}
    assert "browser" in wheres, ("browser module missing from api()", sorted(wheres))
    libs = {r["name"] for r in rows if r["where"] == "library"}
    assert "playwright" in libs, ("playwright not listed as a bundled library", sorted(libs))

    # shot() cost controls (no browser needed -- _encode_shot is a pure helper):
    # a model-bound shot caps its longest edge and re-encodes, so a full-res 2x
    # capture cannot flood context. Build a busy 1746x2406 PNG (the size the
    # friction report measured) and check each path.
    import io as _io
    import os as _os

    from PIL import Image as _Image

    _src = _Image.frombytes("RGB", (1746, 2406), _os.urandom(1746 * 2406 * 3))
    _buf = _io.BytesIO()
    _src.save(_buf, format="PNG")
    _raw = _buf.getvalue()

    # Default model path: JPEG, longest edge -> _SHOT_MAX_DIM (1024).
    _data, _mime = browser._encode_shot(
        _raw, max_dim=browser._SHOT_MAX_DIM, fmt="jpeg", quality=72
    )
    assert _mime == "image/jpeg", _mime
    assert max(_Image.open(_io.BytesIO(_data)).size) == browser._SHOT_MAX_DIM, (
        _Image.open(_io.BytesIO(_data)).size
    )
    # A busy screenshot as JPEG is far smaller than the raw full-res PNG.
    assert len(_data) < len(_raw) // 10, (len(_data), len(_raw))

    # PNG path also downscales the longest edge.
    _pdata, _pmime = browser._encode_shot(_raw, max_dim=1024, fmt="png", quality=72)
    assert _pmime == "image/png", _pmime
    assert max(_Image.open(_io.BytesIO(_pdata)).size) == 1024

    # max_dim=0 + png is an exact passthrough (no needless re-encode).
    _ndata, _nmime = browser._encode_shot(_raw, max_dim=0, fmt="png", quality=72)
    assert _ndata is _raw and _nmime == "image/png", (_nmime, _ndata is _raw)

    # Never raises: junk bytes come back untouched rather than blowing up a shot.
    _gdata, _gmime = browser._encode_shot(b"not an image", max_dim=1024, fmt="jpeg", quality=72)
    assert _gdata == b"not an image" and _gmime == "image/png", (_gmime, _gdata)

    # shot() validates its enum-ish knobs up front.
    import asyncio as _aio
    for _bad in (dict(format="webp"), dict(scale="2x")):
        try:
            _aio.run(browser.shot(**_bad))
        except ValueError:
            pass
        else:
            raise SystemExit(f"shot({_bad}) should raise ValueError")

    # --- live dashboard resource -------------------------------------------
    # A connected browser publishes itself as a live resource: a throttled
    # screenshot of the front tab. No real Chromium needed -- fake the context.
    EP = "http://127.0.0.1:9222"

    class _FakePage:
        url = "https://example.com/"

        last_screenshot_kw = {}

        async def title(self):
            return "Example"

        async def screenshot(self, **_kw):
            _FakePage.last_screenshot_kw = _kw
            return b"NOT-A-REAL-PNG"  # _encode_shot tolerates non-images

    class _FakeCtx:
        def __init__(self, pages):
            self.pages = pages

    class _FakeBrowser:
        def __init__(self):
            self.connected = True

        def is_connected(self):
            return self.connected

    _orig_context = browser.context
    _pages = [_FakePage()]

    async def _fake_context(endpoint=EP):
        return _FakeCtx(_pages)

    browser.context = _fake_context

    # A page renders to an inline <img> with its title/url.
    browser._resource_html_cache.clear()
    _h = _aio.run(browser._resource_html(EP))
    assert "<img" in _h and "example.com" in _h, _h[:200]

    # Passive capture uses device scale, never css: css scale makes Playwright
    # push a per-shot DPR Emulation override that relayouts and visibly flickers
    # the live HiDPI window on every ~1.5s tick of this loop.
    assert _FakePage.last_screenshot_kw.get("scale") == "device", _FakePage.last_screenshot_kw

    # Throttled: a call within the TTL reuses the cache even though the tab list
    # changed underneath it (the screenshot is the expensive part).
    _pages.clear()
    assert _aio.run(browser._resource_html(EP)) == _h

    # No open tabs: a passive placeholder, and never creates a tab.
    browser._resource_html_cache.clear()
    assert "no open tabs" in _aio.run(browser._resource_html(EP))

    # Render never raises: a failing capture becomes an error card.
    async def _boom(endpoint=EP):
        raise RuntimeError("kaboom")

    browser.context = _boom
    browser._resource_html_cache.clear()
    _e = _aio.run(browser._resource_html(EP))
    assert "render failed" in _e and "kaboom" in _e, _e[:200]

    # connect() publishes the resource on a fresh connection; mimic that here.
    browser.context = _fake_context
    _pages[:] = [_FakePage()]
    runtime.resources.clear()
    browser._browsers.clear()
    _fb = _FakeBrowser()
    browser._browsers[EP] = _fb
    _res = browser._register_resource(EP)
    _rid = "browser:" + EP
    assert _res is not None and _rid in runtime.resources, list(runtime.resources)
    assert _res.kind == "browser" and _res.title == "browser · " + EP, (_res.kind, _res.title)
    assert _res.alive() is True
    browser._resource_html_cache.clear()
    assert "<img" in _aio.run(_res.render_html())

    # Keyed by endpoint: a reconnect refreshes the one card, never stacks.
    browser._register_resource(EP)
    assert sum(1 for k in runtime.resources if k == _rid) == 1

    # alive() drops the card once the connection is gone (the sweep then closes it).
    _fb.connected = False
    assert _res.alive() is False
    _fb.connected = True
    browser._browsers.pop(EP)
    assert _res.alive() is False

    # Leave the module clean for any later assertions.
    browser.context = _orig_context
    browser._browsers.clear()
    runtime.resources.clear()
    browser._resource_html_cache.clear()

    print("browser-ok", browser.__version__)
  '';
  browserSmoke =
    pkgs.runCommand "ix-mcp-browser-smoke"
    {
      nativeBuildInputs = [mcpPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe mcpPython} ${browserTestPy} >stdout 2>stderr || {
        echo "ix-mcp browser smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -q '^browser-ok' stdout || {
        echo "ix-mcp browser smoke did not confirm the browser module:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';

  # The clean-vdom contract, exercised against a real (headless) browser. Unlike
  # the launch smoke above, `vdom()` only reads the DOM, so it can run headless on
  # a `data:` fixture with no display and no network: it asserts the filtering
  # (hidden / aria-hidden pruned), the wrapper-chain collapse, that every kept node
  # has geometry and a CSS selector that actually resolves, and that the
  # interactive_only / viewport_only lean modes behave.
  browserVdomTestPy = pkgs.writeText "ix-mcp-browser-vdom-test.py" ''
    # python
    import asyncio
    import sys

    import browser
    from playwright.async_api import async_playwright

    # A fixture page exercising the cleaning rules: landmarks, a collapsible wrapper
    # chain, hidden + aria-hidden subtrees, a named image, and a form. Served as a
    # data: URL so the test needs no network and no on-screen window.
    FIXTURE = (
        "data:text/html," + (
            "<html><head><title>Fixture</title></head><body>"
            "<header><a id='home' href='/'><img alt='Logo'></a>"
            "<nav><a href='/a'>Alpha</a><a href='/b'>Beta</a></nav></header>"
            "<main><h1>Heading</h1><p>Visible paragraph text.</p>"
            "<div><div><div><button id='go' onclick='void 0'>Click me</button></div></div></div>"
            "<form><input type='search' placeholder='Find'><button type='submit'>Go</button></form>"
            "<div style='display:none'><a href='/hidden'>Hidden</a></div>"
            "<span aria-hidden='true'><a href='/aria'>AriaHidden</a></span>"
            "</main></body></html>"
        )
    )


    def names(flat):
        return {n.get("name") for n in flat if not n.get("group")}


    def by_tag(flat, tag):
        return [n for n in flat if n.get("tag") == tag and not n.get("group")]


    async def main():
        pw = await async_playwright().start()
        b = await pw.chromium.launch(
            headless=True, args=["--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage"]
        )
        ctx = await b.new_context(viewport={"width": 1000, "height": 800})
        pg = await ctx.new_page()
        await pg.goto(FIXTURE)

        v = await browser.vdom(pg)

        # It is the documented type, with the page's identity captured.
        assert isinstance(v, browser.Vdom), type(v)
        assert v.title == "Fixture", v.title
        assert v.viewport.get("w") == 1000, v.viewport

        ns = names(v.flat)
        # Hidden (display:none) and aria-hidden subtrees are pruned entirely.
        assert "Hidden" not in ns, ns
        assert "AriaHidden" not in ns, ns
        # Landmarks, heading, controls and the named image survive.
        assert any(n.get("role") == "banner" for n in v.flat), "banner landmark missing"
        assert any(n.get("role") == "navigation" for n in v.flat), "nav landmark missing"
        assert any(n.get("role") == "main" for n in v.flat), "main landmark missing"
        assert any(n.get("role") == "heading" and n.get("name") == "Heading" for n in v.flat), ns
        assert "Logo" in ns, ("named image missing", ns)
        assert any(n.get("name") == "Alpha" and n.get("interactive") for n in v.flat), ns

        # The triple-nested wrapper <div><div><div> around the button is collapsed:
        # the button is reached without a chain of empty group nodes above it.
        btn = next(n for n in v.flat if n.get("tag") == "button" and n.get("name") == "Click me")
        assert btn["depth"] <= 2, ("wrapper chain not collapsed", btn["depth"])

        # Every kept node carries a usable on-screen box and a working CSS selector.
        for n in v.flat:
            if n.get("group"):
                continue
            assert n.get("w", 0) > 0 and n.get("h", 0) > 0, ("no geometry", n)
            sel = n.get("selector")
            assert sel, ("no selector", n)
            assert await pg.query_selector(sel) is not None, ("selector did not resolve", sel)

        # Refs are dense and 1-based; node(ref) round-trips; df/json agree on counts.
        refs = [n["ref"] for n in v.flat if not n.get("group")]
        assert refs == list(range(1, len(refs) + 1)), refs
        assert v.node(refs[-1]) is not None
        n_real = len(refs)
        assert v.df.height == len(v.flat), (v.df.height, len(v.flat))
        assert v.df.filter(v.df["interactive"]).height >= 4  # 4 links + 2 buttons + 1 field

        # The compact glance is bounded and self-describes; the full map lives in .df.
        txt = repr(v)
        assert "Fixture" in txt and "nodes" in txt, txt[:200]

        # interactive_only drops body text but keeps the controls.
        vi = await browser.vdom(pg, interactive_only=True)
        nsi = names(vi.flat)
        assert "Visible paragraph text." not in nsi, nsi
        assert any(n.get("name") == "Alpha" for n in vi.flat), nsi

        # viewport_only keeps only on-screen nodes (all fixture nodes are on screen,
        # so it must still find the controls -- and never error).
        vvp = await browser.vdom(pg, viewport_only=True)
        assert any(n.get("interactive") for n in vvp.flat), "viewport_only lost controls"

        await b.close()
        await pw.stop()
        print("vdom-ok", browser.__version__, n_real, "nodes")


    # A sandboxed headless chromium occasionally tears down mid-run
    # (TargetClosedError); that is environment flake, not a vdom regression, so
    # retry the whole run a couple of times before failing the gate.
    for attempt in range(3):
        try:
            asyncio.run(main())
            break
        except Exception as exc:
            if attempt == 2 or "closed" not in str(exc).lower():
                raise
            print(f"retry {attempt + 1}: transient browser teardown: {exc}", file=sys.stderr)
  '';
  browserVdomSmoke =
    pkgs.runCommand "ix-mcp-browser-vdom-smoke"
    {
      nativeBuildInputs = [mcpPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      # `vdom()` launches a (headless) browser, so point Playwright at the bundled
      # browser bundle -- the bare mcpPython has no wrapper to set this (only the
      # `ix-mcp` entrypoint does).
      export PLAYWRIGHT_BROWSERS_PATH=${lib.escapeShellArg playwrightBrowsers}
      export FONTCONFIG_FILE=${fontsConf}
      ${lib.getExe mcpPython} ${browserVdomTestPy} >stdout 2>stderr || {
        echo "ix-mcp browser vdom smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -q '^vdom-ok' stdout || {
        echo "ix-mcp browser vdom smoke did not confirm the clean vdom:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';

  # Property-based (Hypothesis) tests for the vdom()/read() snapshot helpers
  # (packages/mcp/tests/test_vdom_properties.py): they generate random HTML
  # bodies and assert selector integrity, exclusion of hidden/script/style
  # subtrees, name clamping, ref contiguity, determinism, the interactive_only
  # subset relation, geometry, and read()/vdom() agreement against a real
  # headless Chromium. Like browserVdomSmoke, vdom() only reads the DOM, so it
  # runs headless on set_content fixtures in the sandbox with no display or
  # network. The interpreter is mcpPython (the bundled browser module +
  # playwright) plus pytest and hypothesis, which the bare mcpPython omits.
  # Reuses the full mcp module set (which includes the asn1-pinned pymobiledevice3
  # override), so it must build from the same asn1-pinned interpreter.
  vdomTestPython = mcpPythonInterp.withPackages (
    ps:
      mcpPythonPackages ps
      ++ [
        ps.pytest
        ps.hypothesis
      ]
  );
  vdomPropertiesSource = builtins.path {
    name = "ix-mcp-vdom-properties-test";
    path = ./tests/test_vdom_properties.py;
  };
  vdomPropertiesSmoke =
    pkgs.runCommand "ix-mcp-vdom-properties-smoke"
    {
      nativeBuildInputs = [vdomTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      # `vdom()` launches a (headless) browser; point Playwright at the bundled
      # browser bundle (no wrapper sets it for the bare interpreter).
      export PLAYWRIGHT_BROWSERS_PATH=${lib.escapeShellArg playwrightBrowsers}
      export FONTCONFIG_FILE=${fontsConf}
      # Copy the test into a writable dir so pytest collects it as a plain file
      # (a bare store path of a single .py is read by pytest as a directory).
      cp ${vdomPropertiesSource} "$TMPDIR/test_vdom_properties.py"
      ${lib.getExe vdomTestPython} -m pytest "$TMPDIR/test_vdom_properties.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp vdom property tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';

  # Interactive input: the browser -> kernel write path behind interactive
  # resources (packages/mcp/tests/test_inputs.py). Covers the store queue, the
  # dashboard `/api/input` gate + CORS (over a real aiohttp TestServer), and the
  # kernel-side drain into an awaiting `Input` / `ask`. Needs the full mcp
  # interpreter (ix_notebook_mcp + aiohttp) plus pytest, which bare mcpPython omits.
  inputsTestPython = mcpPythonInterp.withPackages (
    ps:
      mcpPythonPackages ps
      ++ [
        ps.pytest
      ]
  );
  inputsTestSource = builtins.path {
    name = "ix-mcp-inputs-test";
    path = ./tests/test_inputs.py;
  };
  inputsTests =
    pkgs.runCommand "ix-mcp-inputs-tests"
    {
      nativeBuildInputs = [inputsTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${inputsTestSource} "$TMPDIR/test_inputs.py"
      ${lib.getExe inputsTestPython} -m pytest "$TMPDIR/test_inputs.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp inputs tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';

  # Background-task failure reporting (packages/mcp/tests/test_task_errors.py):
  # a fire-and-forget task that dies with an unretrieved exception must be
  # reported at completion into `task_errors` (asyncio's own warning only fires
  # at GC, and never for a task a namespace variable keeps alive -- the exact
  # watcher pattern that starved monitors silently on 2026-07-02), plus the
  # `Result.output` alias that AttributeError'd that watcher.
  taskErrorsTestSource = builtins.path {
    name = "ix-mcp-task-errors-test";
    path = ./tests/test_task_errors.py;
  };
  taskErrorsTests =
    pkgs.runCommand "ix-mcp-task-errors-tests"
    {
      nativeBuildInputs = [channelTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${taskErrorsTestSource} "$TMPDIR/test_task_errors.py"
      ${lib.getExe channelTestPython} -m pytest "$TMPDIR/test_task_errors.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp task-errors tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';

  # Redundant-read tracking (packages/mcp/tests/test_readstats.py, index#1924):
  # the per-session tracker's core contract (same-content re-read is redundant,
  # changed content is not, a different path is not, counters are per-session) and
  # the exact `mcp_read_stats` journald line the ix fleet pipeline parses. Only
  # imports `ix_notebook_mcp.readstats` (pure stdlib), so it reuses the typecheck
  # interpreter, which already carries pytest.
  readStatsTestSource = builtins.path {
    name = "ix-mcp-readstats-test";
    path = ./tests/test_readstats.py;
  };
  readStatsTests =
    pkgs.runCommand "ix-mcp-readstats-tests"
    {
      nativeBuildInputs = [typecheckTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${readStatsTestSource} "$TMPDIR/test_readstats.py"
      ${lib.getExe typecheckTestPython} -m pytest "$TMPDIR/test_readstats.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp readstats tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';

  # The Claude Code channel + interactive resource actions
  # (packages/mcp/tests/test_channel.py): the store outbox/events queues, the
  # kernel's notify() + action dispatch, the transport pump emitting
  # notifications/claude/channel, the reply tool, and the dashboard's SSE feed.
  # Same interpreter needs as inputsTests (ix_notebook_mcp + aiohttp + the mcp
  # SDK) plus pytest.
  channelTestPython = mcpPythonInterp.withPackages (
    ps:
      mcpPythonPackages ps
      ++ [
        ps.pytest
      ]
  );
  channelTestSource = builtins.path {
    name = "ix-mcp-channel-test";
    path = ./tests/test_channel.py;
  };
  channelTests =
    pkgs.runCommand "ix-mcp-channel-tests"
    {
      nativeBuildInputs = [channelTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${channelTestSource} "$TMPDIR/test_channel.py"
      ${lib.getExe channelTestPython} -m pytest "$TMPDIR/test_channel.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp channel tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';

  # End-to-end browser proof of the interactive-input path: a real headless
  # Chromium mounts an `Input`'s HTML in a sandboxed, opaque-origin srcdoc iframe
  # (as HtmlBody.svelte does), clicks the button, and the cross-origin `ixSubmit`
  # fetch must reach the real aiohttp `/api/input` and drain into the awaiting
  # channel (packages/mcp/tests/test_input_browser.py). Same interpreter + bundled
  # browser as the vdom smoke, plus pytest.
  inputBrowserTestPython = mcpPythonInterp.withPackages (
    ps:
      mcpPythonPackages ps
      ++ [
        ps.pytest
      ]
  );
  inputBrowserTestSource = builtins.path {
    name = "ix-mcp-input-browser-test";
    path = ./tests/test_input_browser.py;
  };
  inputBrowserSmoke =
    pkgs.runCommand "ix-mcp-input-browser-smoke"
    {
      nativeBuildInputs = [inputBrowserTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      export PLAYWRIGHT_BROWSERS_PATH=${lib.escapeShellArg playwrightBrowsers}
      export FONTCONFIG_FILE=${fontsConf}
      cp ${inputBrowserTestSource} "$TMPDIR/test_input_browser.py"
      ${lib.getExe inputBrowserTestPython} -m pytest "$TMPDIR/test_input_browser.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp input browser smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';

  # The whole Svelte resource path (packages/mcp/tests/test_svelte.py): the
  # nix-built svelte-bundle CLI compiles a Svelte 5 component, a real sandboxed
  # iframe renders the kernel-embedded state, `act` rides the real /api/input,
  # and the action_result re-renders the page. Same interpreter + browser needs
  # as inputBrowserSmoke, plus the CLI on IX_SVELTE_BUNDLE_BIN.
  svelteBundled = importTest "svelte" "import svelte; print('svelte-ok', callable(svelte.bundle), callable(svelte.component))";
  svelteTestPython = mcpPythonInterp.withPackages (
    ps:
      mcpPythonPackages ps
      ++ [
        ps.pytest
      ]
  );
  svelteTestSource = builtins.path {
    name = "ix-mcp-svelte-test";
    path = ./tests/test_svelte.py;
  };
  svelteTests =
    pkgs.runCommand "ix-mcp-svelte-tests"
    {
      nativeBuildInputs = [svelteTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      export PLAYWRIGHT_BROWSERS_PATH=${lib.escapeShellArg playwrightBrowsers}
      export FONTCONFIG_FILE=${fontsConf}
      export IX_SVELTE_BUNDLE_BIN=${lib.escapeShellArg (lib.getExe svelteBundleBin)}
      cp ${svelteTestSource} "$TMPDIR/test_svelte.py"
      ${lib.getExe svelteTestPython} -m pytest "$TMPDIR/test_svelte.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp svelte tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';

  screenBundled = importTest "screen" "import screen; print('screen-ok', all(callable(getattr(screen, n)) for n in ('capture', 'click', 'write', 'press', 'key_down', 'key_up', 'apps', 'frontmost', 'launch', 'activate', 'terminate', 'accessibility_trusted')))";
  coreLocationBundled = importTest "corelocation" "import CoreLocation; print('corelocation-ok', callable(CoreLocation.CLLocationManager.alloc))";
  scriptingBridgeBundled = importTest "scriptingbridge" "import ScriptingBridge; print('scriptingbridge-ok', callable(ScriptingBridge.SBApplication.applicationWithBundleIdentifier_))";
  mapsBundled = importTest "maps" "import maps, MapKit; print('maps-ok', all(callable(getattr(maps, n)) for n in ('nearby', 'geocode', 'reverse_geocode')), callable(MapKit.MKLocalSearch.alloc))";
  vmkitBundled = importTest "vmkit" "import vmkit; print('vmkit-ok', callable(vmkit.boot_linux), callable(vmkit.drive), callable(vmkit.screenshot))";
  imessageBundled = importTest "imessage" "import imessage; print('imessage-ok', all(callable(getattr(imessage, n)) for n in ('messages', 'chats', 'contacts', 'send')))";
  ghosttyBundled = importTest "ghostty" "import ghostty; print('ghostty-ok', all(callable(getattr(ghostty, n)) for n in ('surfaces', 'my_tty', 'my_surface', 'close', 'close_me', 'focus', 'activate', 'is_running')), ghostty.__version__)";
  xBundled = importTest "x" "import x; print('x-ok', callable(x.posts), x.__version__)";
  meshBundled = importTest "mesh" "import mesh, asyncio; print('mesh-ok', all(asyncio.iscoroutinefunction(getattr(mesh, n)) for n in ('peers', 'sessions')), mesh.__version__)";
  linearBundled = importTest "linear" "import linear; print('linear-ok', all(callable(getattr(linear, n)) for n in ('issue', 'issue_update', 'issue_create', 'issue_search', 'comment_create', 'project_create')), linear.__version__)";
  notionBundled = importTest "notion" "import notion, asyncio; print('notion-ok', all(asyncio.iscoroutinefunction(getattr(notion, n)) for n in ('search', 'page', 'blocks', 'db_query', 'page_create', 'blocks_append', 'page_update')), notion.__version__)";
  notionTestPython = pkgs.python3.withPackages (ps: [
    ps.pytest
    ps.httpx
    ps.polars
    ps.pydantic
    notionModule
  ]);
  notionTestSource = builtins.path {
    name = "ix-mcp-notion-test";
    path = ./tests/test_notion.py;
  };
  notionTests =
    pkgs.runCommand "ix-mcp-notion-tests"
    {
      nativeBuildInputs = [notionTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${notionTestSource} "$TMPDIR/test_notion.py"
      ${lib.getExe notionTestPython} -m pytest "$TMPDIR/test_notion.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp notion tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
  nuBundled = importTest "nu" "import nu; print('nu-ok', callable(nu), callable(nu.value), nu.NuError.__name__ == 'NuError', nu.__version__)";
  # Behavior tests for the embedded nushell engine: the normalization matrix,
  # persistent REPL state, native datetime/duration crossing, the NuError
  # diagnostic surface, `exit` safety, and interrupt-based timeout. Everything
  # runs in-process against the real engine, so the sandbox needs no nushell
  # binary and no network.
  nuTestPython = pkgs.python3.withPackages (ps: [
    ps.pytest
    ps.polars
    nuPyModule
  ]);
  nuTestSource = builtins.path {
    name = "ix-mcp-nu-test";
    path = ./tests/test_nu.py;
  };
  nuTests =
    pkgs.runCommand "ix-mcp-nu-tests"
    {
      nativeBuildInputs = [nuTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${nuTestSource} "$TMPDIR/test_nu.py"
      ${lib.getExe nuTestPython} -m pytest "$TMPDIR/test_nu.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp nu tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
  noxAutotriageBundled = importTest "nox-autotriage" "import nox_autotriage; print('nox-autotriage-ok', callable(nox_autotriage.findings_from_conformance))";
  linearTriageTestPython = pkgs.python3.withPackages (ps: [
    ps.pytest
    ps.httpx
    ps.pydantic
    linearModule
  ]);
  linearTriageTestSource = builtins.path {
    name = "ix-mcp-linear-triage-test";
    path = ./tests/test_linear_triage.py;
  };
  linearTriageTests =
    pkgs.runCommand "ix-mcp-linear-triage-tests"
    {
      nativeBuildInputs = [linearTriageTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${linearTriageTestSource} "$TMPDIR/test_linear_triage.py"
      ${lib.getExe linearTriageTestPython} -m pytest "$TMPDIR/test_linear_triage.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp linear triage tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
  noxAutotriageTestPython = pkgs.python3.withPackages (ps: [
    ps.pytest
    ps.httpx
    ps.pydantic
    linearModule
    noxAutotriageModule
  ]);
  noxAutotriageTestSource = builtins.path {
    name = "ix-mcp-nox-autotriage-test";
    path = ./tests/test_nox_autotriage.py;
  };
  noxAutotriageTestFixtures = builtins.path {
    name = "ix-mcp-nox-autotriage-test-fixtures";
    path = ./tests/fixtures;
  };
  noxAutotriageTests =
    pkgs.runCommand "ix-mcp-nox-autotriage-tests"
    {
      nativeBuildInputs = [noxAutotriageTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      mkdir -p "$TMPDIR/fixtures"
      cp ${noxAutotriageTestSource} "$TMPDIR/test_nox_autotriage.py"
      cp -r ${noxAutotriageTestFixtures}/. "$TMPDIR/fixtures/"
      ${lib.getExe noxAutotriageTestPython} -m pytest "$TMPDIR/test_nox_autotriage.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp nox-autotriage tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';

  # The `iphone` helper imports in the real interpreter and exposes its surface.
  # Cross-platform: pulls in the vendored pymobiledevice3 CLI, so it also proves
  # that uv closure builds on Linux CI.
  iphoneBundled = importTest "iphone" "import iphone; print('iphone-ok', all(callable(getattr(iphone, n)) for n in ('devices', 'apps', 'screenshot', 'launch', 'start_tunneld', 'tap', 'swipe')))";

  # Device-free behaviour tests (exports, async signatures, explicit type hints,
  # CLI-path resolution, the sudo guard, the no-device error).
  iphoneTestPython = pkgs.python3.withPackages (ps: [
    ps.pytest
    ps.polars
    iphoneModule
  ]);
  iphoneTestSource = builtins.path {
    name = "ix-mcp-iphone-test";
    path = ./tests/test_iphone.py;
  };
  iphoneTests =
    pkgs.runCommand "ix-mcp-iphone-tests"
    {
      nativeBuildInputs = [iphoneTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${iphoneTestSource} "$TMPDIR/test_iphone.py"
      ${lib.getExe iphoneTestPython} -m pytest "$TMPDIR/test_iphone.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp iphone tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';

  # Network-free unit tests for the `slack` helper: module shape plus that
  # `send` builds the right chat.postMessage params for top-level vs. in-thread
  # replies (stubbing the one network primitive).
  slackTestPython = pkgs.python3.withPackages (ps: [
    ps.pytest
    ps.polars
    ps.pydantic
    slackModule
  ]);
  slackTestSource = builtins.path {
    name = "ix-mcp-slack-test";
    path = ./tests/test_slack.py;
  };
  slackTests =
    pkgs.runCommand "ix-mcp-slack-tests"
    {
      nativeBuildInputs = [slackTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${slackTestSource} "$TMPDIR/test_slack.py"
      ${lib.getExe slackTestPython} -m pytest "$TMPDIR/test_slack.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp slack tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';

  # Network-free unit tests for the federated-resources bridge: every path of
  # `resources_bridge` (list/read/act, peer-flag assembly, not-found -> -32002,
  # graceful empty/clear-error when `ix-resource-cli` is absent) driven against a
  # STUB `ix-resource-cli` script on PATH plus a nonexistent-binary path -- no
  # real CLI or peer needed. The bridge lives in the `ix_notebook_mcp` server
  # package, so the test imports that module (bundled here) rather than a `src/*`
  # helper; `bash` is on PATH for the stub script's shebang.
  resourcesBridgeTestPython = pkgs.python3.withPackages (ps: [
    ps.pytest
    ps.pydantic
    ixNotebookMcpModule
  ]);
  resourcesBridgeTestSource = builtins.path {
    name = "ix-mcp-resources-bridge-test";
    path = ./tests/test_resources_bridge.py;
  };
  resourcesBridgeTests =
    pkgs.runCommand "ix-mcp-resources-bridge-tests"
    {
      nativeBuildInputs = [
        resourcesBridgeTestPython
        pkgs.bash
      ];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${resourcesBridgeTestSource} "$TMPDIR/test_resources_bridge.py"
      ${lib.getExe resourcesBridgeTestPython} -m pytest "$TMPDIR/test_resources_bridge.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp resources-bridge tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';

  # Network-free tests for the tailnet auto-mesh (index#1787): the `/mesh`
  # route and its skip paths (IX_MCP_MESH=0, no tailscale IP, bind conflict),
  # the bundled `mesh` module's peer sweep against a STUB `tailscale` script
  # plus a real loopback server, and fleet.connect's zero-config Ray-head
  # probe against a fake GCS listener. asyncssh rides along because importing
  # `fleet` pulls it; `bash` backs the stub script's shebang. The session-label
  # tests import `ix_notebook_mcp.tools`, whose import chain needs the mcp SDK
  # (pydantic rides along) and nbformat (via `outputs`).
  meshTestPython = pkgs.python3.withPackages (ps: [
    ps.pytest
    ps.aiohttp
    ps.httpx
    ps.polars
    ps.asyncssh
    ps.mcp
    ps.nbformat
    ixNotebookMcpModule
    meshModule
    fleetModule
  ]);
  meshTestSource = builtins.path {
    name = "ix-mcp-mesh-test";
    path = ./tests/test_mesh.py;
  };
  meshTests =
    pkgs.runCommand "ix-mcp-mesh-tests"
    {
      nativeBuildInputs = [
        meshTestPython
        pkgs.bash
      ];
      strictDeps = true;
      # The tests bind loopback sockets (a real mesh server + a fake GCS
      # listener); the darwin sandbox denies all binds without this. Linux
      # sandboxes already provide a private loopback, so it is a no-op there.
      __darwinAllowLocalNetworking = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${meshTestSource} "$TMPDIR/test_mesh.py"
      ${lib.getExe meshTestPython} -m pytest "$TMPDIR/test_mesh.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp mesh tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests =
          {
            inherit
              strictTypecheck
              tuiBundled
              htpyBundled
              searchBundled
              astlogBundled
              fsearchBundled
              dataLibsBundled
              gmailLibsBundled
              exaBundled
              cursorSdkBundled
              googleAuthBundled
              ixGoogleBundled
              slackBundled
              slackTests
              resourcesBridgeTests
              meshBundled
              meshTests
              beeperBundled
              requirementsSmoke
              engineBundled
              serverTools
              evalSmoke
              runtimeSmoke
              typecheckSmoke
              sessionSmoke
              sessionIdentitySmoke
              feedSmoke
              apiSmoke
              inputsTests
              channelTests
              taskErrorsTests
              readStatsTests
              inputBrowserSmoke
              svelteBundled
              svelteTests
              wedgeSmoke
              richSmoke
              yieldSmoke
              bindingsSmoke
              bindDefaultSmoke
              sshAuthSockSmoke
              dashboardLauncherSmoke
              viewSmoke
              nixSmoke
              fleetSmoke
              fleetClusterSmoke
              shSmoke
              worktreeSmoke
              browserSmoke
              browserVdomSmoke
              vdomPropertiesSmoke
              xBundled
              nuBundled
              nuTests
              linearBundled
              linearTriageTests
              notionBundled
              notionTests
              noxAutotriageBundled
              noxAutotriageTests
              iphoneBundled
              iphoneTests
              ;
          }
          // lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
            inherit
              screenBundled
              coreLocationBundled
              scriptingBridgeBundled
              mapsBundled
              mapsSmoke
              vmkitBundled
              vmkitResourceSmoke
              imessageBundled
              imessageSmoke
              ghosttyBundled
              ghosttySmoke
              ;
          };
      };
  })
