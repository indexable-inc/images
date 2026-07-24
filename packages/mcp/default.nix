{
  ix,
  lib,
  # The fork client rather than stock `pkgs.nix`; see packages/yc/default.nix
  # for why an updater must not pull nixpkgs' nix into its closure. Empty on
  # the overlay path, which omits the updateScript anyway.
  repoPackages ? {},
  updateScriptWriter ? null,
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

  # PyPI pins (version + URL + SRI hash) for the interpreter overrides below,
  # in the sibling pins.json (repo policy: no inline hash literals in tracked
  # .nix). `nix run .#mcp.updateScript` joins the registry update DAG and
  # refreshes normal PyPI sdist pins from the JSON API. pins.json policy markers
  # are `prefetch = "manual"` for hash-mode holds, `hold` for version holds, and
  # `track` for version-line tracking, so the updater skips or narrows pins
  # loudly instead of guessing.
  pypiPins = ix.pins.loadPins ./pins.json;
  updateScript =
    if updateScriptWriter == null
    then null
    else
      import ./update.nix {
        nix = repoPackages.nix-ix;
        writeNushellApplication = updateScriptWriter;
      };
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
    path = ix.paths.packagesRoot + "/search-py/python";
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
    path = ix.paths.packagesRoot + "/astlog/py/python";
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
  # index. Unlike `astlogModule` above, the site tree comes from
  # `ix.unibind.build`: unibind-generated stub + `py.typed` merged with the
  # hand-written wrapper, cdylib from the shared workspace graph. Same
  # arguments as packages/scipql/py/default.nix (the wheel); keep the two
  # call sites in sync. (The CLI bakes in rust-analyzer/souffle; the kernel
  # module exposes facts/query/fix/rename over an already-built index.scip.)
  scipqlModule =
    (ix.unibind.build {
      crate = "scipql-py";
      targets.py = {
        package = "scipql";
        pythonSource = builtins.path {
          name = "scipql-py-python-source";
          path = ix.paths.packagesRoot + "/scipql/py/python";
        };
        pythonPackages = ps: [ps.polars];
      };
    }).py.module;

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

  # One privacy boundary shared by every helper that can expose a signed-in
  # user's personal account data. Keeping the IX_MCP_SHARED policy here makes
  # the refusal semantics impossible to drift between integrations.
  privateSessionSource = builtins.path {
    name = "ix-mcp-private-session-source";
    path = ./src/private_session.py;
  };
  privateSessionModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-private-session-module"
    {
      strictDeps = true;
      meta.description = "Shared private-session guard for personal MCP integrations";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}"
      mkdir -p "$site"
      install -Dm644 ${privateSessionSource} "$site/private_session.py"
    ''
  );

  # The distiller's transcript reader (packages/distiller), the single
  # Python-side owner of the Claude transcript schema, bundled from that
  # package's own passthru so the module recipe is not duplicated here.
  # `claude_history` imports `distiller.transcripts` (stdlib-only); the
  # distiller's optional deps (pyarrow/boto3) stay out of this interpreter.
  distillerModule = pkgs.ix-distiller.passthru.pythonModule;
  # The vmkit binary `vmkit` spawns. Darwin-only; referenced lazily so a Linux
  # mcp build never forces it.
  vmkitBin = ix.rustWorkspace.units.binaries.vmkit;

  # The gcal binary the calendar tools spawn with --json: the CLI surface of
  # the google-calendar crate (packages/google/calendar), so the MCP binding
  # carries no calendar logic of its own (RFC 0003).
  gcalBin = ix.rustWorkspace.units.binaries.gcal;

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

  # The `screen` helper is macOS-only, so its dependencies join the interpreter
  # only on Darwin. `pyobjc-framework-Quartz` is the maintained CoreGraphics
  # binding the helper wraps; Pillow (already transitive via matplotlib) carries
  # the PIL image type capture returns. `coreLocationModule` adds the Core
  # Location binding so location reads work out of the box, and
  # `scriptingBridgeModule` the Scripting Bridge binding for app automation.
  # Split from the darwin-only first-party modules below so the per-module test
  # base (third-party only) can reuse these without dragging module sources in.
  darwinFrameworkPackages = ps:
    lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
      ps.pyobjc-framework-Quartz
      coreLocationModule
      scriptingBridgeModule
      mapKitModule
    ];
  # The darwin-only bundled first-party modules: the discovered module dirs
  # whose module.nix declares `darwinOnly = true`.
  darwinBundledModules = lib.optionals pkgs.stdenv.hostPlatform.isDarwin (
    lib.mapAttrsToList (_: entry: entry.module) (
      lib.filterAttrs (_: entry: entry.darwinOnly or false) bundledEntries
    )
  );
  # `embed` (code embeddings, index#3417) infers on torch/MPS, so its
  # heavyweight runtime joins the interpreter only on Darwin; the module
  # itself is bundled everywhere and imports these lazily with a clear
  # error where they are absent. `torch` substitutes from the official
  # cache. `sentence-transformers` is overridden because the stock package
  # folds every optional-dependencies extra into its nativeCheckInputs and
  # the `audio` extra carries phonemizer -> dlinfo, which this nixpkgs
  # marks broken, refusing evaluation outright. The extras are test-only:
  # drop the test run rather than allowlist a broken leaf; runtime
  # dependencies are untouched.
  darwinTorchPackages = ps:
    lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
      ps.torch
      (ps.sentence-transformers.overridePythonAttrs (_: {
        doCheck = false;
        nativeCheckInputs = [];
      }))
    ];
  # Concatenated in the shipped env's historical order (frameworks, modules,
  # torch) so the interpreter's buildEnv input order -- and store path -- do
  # not move.
  darwinExtraPackages = ps:
    darwinFrameworkPackages ps
    ++ darwinBundledModules
    ++ darwinTorchPackages ps;

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

  # Bundled first-party modules, one directory per module (issue #3928): every
  # directory under this package carrying a `module.nix` marker is a bundled
  # module, its Nix definition living next to its Python source
  # (src/<name>/module.nix, plus ix_notebook_mcp/module.nix for the server
  # package itself). Same discovery idiom as packages/registry.nix's
  # package.nix walk. Each module.nix is a function returning
  # `{ module, darwinOnly ? false, tests ? {} }` (embed also returns `cli`,
  # which the assembly surfaces as `passthru.embedCli`); its formals are fed by
  # intersection from `moduleScope`, so a module file declares exactly the
  # slice of this assembly it uses: shared helpers, third-party pins, and
  # sibling modules under their `<name>Module` bindings (which is how a
  # `passthru.ixFirstPartyDeps` declaration travels with its module).
  discoveredModules = ix.discoverTree {
    root = ./.;
    requiredFiles = ["module.nix"];
  };
  # A bundled module directory as a Python source tree: everything except the
  # module.nix marker itself, so editing a module's Nix definition does not
  # invalidate its Python module output.
  bundledSource = {
    name,
    path,
  }:
    builtins.path {
      inherit name path;
      filter = filePath: _type: builtins.baseNameOf filePath != "module.nix";
    };
  # Module directories are snake_case on disk; the scope binds each module drv
  # under the camelCase `<name>Module` binding the definitions historically
  # used (src/nox_autotriage -> noxAutotriageModule).
  camelName = name: let
    parts = lib.splitString "_" name;
  in
    builtins.head parts + lib.concatMapStrings lib.toSentenceCase (builtins.tail parts);
  moduleScope =
    {
      inherit
        lib
        pkgs
        bundledSource
        importTest
        bundledTestPython
        bundledTestPythonWith
        serverTestPython
        typecheckTestPython
        playwrightBrowsers
        fontsConf
        svelteBundleBin
        tuiModule
        nuPyModule
        privateSessionModule
        distillerModule
        # The shipped interpreter, for module-owned artifacts that must run on
        # the exact env the kernels run (embed's CLI). Anything built over it
        # rides the whole bundled closure, so per-module tests should prefer
        # `bundledTestPython` unless the artifact under test IS that closure.
        mcpPython
        ;
      testsRoot = ./tests;
    }
    // lib.mapAttrs' (name: entry: lib.nameValuePair "${camelName name}Module" entry.module) bundledEntries;
  bundledEntries =
    lib.mapAttrs (
      _: discovered: let
        moduleFn = import (discovered.path + "/module.nix");
      in
        moduleFn (builtins.intersectAttrs (builtins.functionArgs moduleFn) moduleScope)
    )
    discoveredModules;
  bundledModule = name:
    (bundledEntries."${name}" or (throw "ix-mcp: no bundled module directory named '${name}'")).module;
  # The assembly below reaches a few discovered modules by name.
  ixNotebookMcpModule = bundledModule "ix_notebook_mcp";
  claudeHistoryModule = bundledModule "claude_history";
  # The embed battery's CLI (index#3905), defined next to its module in
  # src/embed/module.nix and surfaced as `nix run .#embed` through
  # lib/per-system.nix.
  embedCli = bundledEntries.embed.cli;
  bundledModuleTests = lib.mergeAttrsList (
    map (entry: entry.tests or {}) (builtins.attrValues bundledEntries)
  );

  # The interpreter the wrapper pins. Sessions build their venv from this with
  # `--system-site-packages`, so `tui`, `search`, `exa_py`, numpy, polars
  # (incl. Postgres via psycopg + SQLAlchemy), duckdb, httpx, htpy, and playwright
  # are importable by default while an in-session `pip install` still writes to
  # the per-session venv.
  # The bundled-package set the pinned interpreter carries, split in two: the
  # third-party base (PyPI/nixpkgs packages, which change rarely) and the
  # first-party module block (repo sources, which churn). The per-module test
  # envs below reuse the third-party base plus only the modules under test;
  # the shipped env concatenates both, in the historical order, so the store
  # path of the shipped interpreter does not move.
  mcpThirdPartyPackages = ps: [
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
    # githubkit: typed async GitHub API client generated from GitHub's OpenAPI
    # spec, so a session does GitHub reads/writes as direct API calls instead
    # of `gh` subprocesses (index#3258: a REST call answers in under a second
    # where each `gh` fork costs several; `gh` stays for auth bootstrap via
    # `gh auth token`).
    ps.githubkit
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
    # claude-agent-sdk: the Claude Agent SDK the `fabric.claude` session
    # helper drives (streaming input + native interrupt over the Claude Code
    # CLI subprocess), replacing PTY scraping for programmatic claudes.
    ps.claude-agent-sdk
  ];
  # The bundled first-party modules: the externally-sourced ones defined above
  # (their Python lives in other packages' trees), then every discovered
  # module directory that is not darwin-only, in directory-name order. Each
  # module declares its first-party imports as `passthru.ixFirstPartyDeps` on
  # its own definition (issue #3897).
  mcpBundledModules =
    [
      tuiModule
      searchModule
      nuPyModule
      astlogModule
      scipqlModule
      flecsQueryModule
      privateSessionModule
      ixGoogleModule
      distillerModule
    ]
    ++ lib.mapAttrsToList (_: entry: entry.module) (
      lib.filterAttrs (_: entry: !(entry.darwinOnly or false)) bundledEntries
    );
  mcpPythonPackages = ps:
    mcpThirdPartyPackages ps
    ++ mcpBundledModules
    ++ [
      # pymobiledevice3 9.27.0 (defined above), whose CLI the bundled `iphone`
      # module drives. The wrapper resolves the `pymobiledevice3` console
      # script next to the interpreter, so both ride in the same env.
      pymobiledevice3_927
    ]
    # Ray's `client` extra (grpcio): `fabric.remote` attaches to the fleet head
    # over `ray://` (Ray Client), which plain ps.ray refuses without it. Derived
    # from ray's own extra rather than named: pysparkConnect happens to carry
    # grpcio today, but reaching the cluster must not hinge on spark.
    ++ ps.ray.optional-dependencies.client
    ++ darwinExtraPackages ps;
  mcpPython = mcpPythonInterp.withPackages mcpPythonPackages;

  # Per-module test envs (issue #3897). Every bundled module declares its
  # first-party imports as `passthru.ixFirstPartyDeps` on its own definition;
  # the transitive closure over those declarations is the single source of
  # truth for what a module's tests need, so editing one module re-runs only
  # the tests whose closure contains it instead of every bundled test. The
  # import graph is cyclic where modules integrate with the kernel runtime in
  # both directions (`sh` <-> `ix_notebook_mcp`), which genericClosure's
  # key-dedup handles and a propagatedBuildInputs edge could not.
  firstPartyClosure = modules:
    map (entry: entry.value) (builtins.genericClosure {
      startSet =
        map (m: {
          key = m.outPath;
          value = m;
        })
        modules;
      operator = entry:
        map (m: {
          key = m.outPath;
          value = m;
        }) (entry.value.ixFirstPartyDeps or []);
    });
  # The third-party base every per-module test env shares: exactly the shipped
  # interpreter's non-first-party packages, so a module's PyPI/nixpkgs deps
  # resolve in tests precisely as they do at runtime, while edits to sibling
  # first-party modules cannot invalidate the env.
  mcpTestBasePackages = ps:
    mcpThirdPartyPackages ps
    ++ [pymobiledevice3_927]
    ++ ps.ray.optional-dependencies.client
    ++ darwinFrameworkPackages ps
    ++ darwinTorchPackages ps;
  bundledTestPythonWith = extras: modules:
    mcpPythonInterp.withPackages (
      ps:
        mcpTestBasePackages ps
        ++ firstPartyClosure modules
        ++ extras ps
    );
  bundledTestPython = bundledTestPythonWith (_: []);
  # The kernel/server package's own test env, shared by every smoke test that
  # imports or boots ix_notebook_mcp.
  serverTestPython = bundledTestPython [ixNotebookMcpModule];

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

  # `ty` (astral-sh's Rust type checker) drives the per-cell static type check the
  # kernel runs before every `python_exec` cell (see ix_notebook_mcp/typecheck.py).
  # It is a nix-provided dependency baked onto the wrapper env (IX_MCP_TY_BIN), not
  # fetched at runtime, and it checks against `mcpPython` (IX_MCP_TY_PYTHON) so a
  # cell importing a bundled module resolves that module's real types.
  tyBin = lib.getExe pkgs.ty;

  # TLS trust for every shelled-out client in the kernel (issue #2429): the
  # nix-built curl/git carry no baked-in system CA path on darwin and the
  # launchd/user environment provides none, so `^curl https://...` inside nu()
  # failed verification (exit 60) while httpx in the same kernel worked
  # (Python carries certifi). set-default, not set: an operator-provided
  # bundle (a corporate CA) must still win.
  caBundle = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";

  # The fabric env handshake + Ray darwin-cluster gates (index#3192), baked
  # onto both wrappers from the one owner of the format (lib/fabric.nix):
  # IX_FABRIC_ENV names this driver's `fabric_env:<tag>` resource, which
  # fabric.remote compares against the target node's advertised resource at
  # submit, and the RAY_ENABLE_* darwin gates live ONLY on these wrappers and
  # the node daemons, never in user shells.
  fabricWrapperFlags = lib.concatStringsSep " " (
    lib.mapAttrsToList (name: value: "--set ${name} ${lib.escapeShellArg value}")
    (ix.fabric.kernelEnv mcpPythonInterp)
  );

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
        ${fabricWrapperFlags} \
        --set PLAYWRIGHT_BROWSERS_PATH ${lib.escapeShellArg playwrightBrowsers} \
        --set IX_SVELTE_BUNDLE_BIN ${lib.escapeShellArg (lib.getExe svelteBundleBin)} \
        --set IX_GCAL_BIN ${lib.escapeShellArg "${gcalBin}/bin/gcal"} \
        --set SCIPQL_SOUFFLE ${lib.escapeShellArg (lib.getExe' pkgs.souffle "souffle")} \
        --set IX_MCP_TY_BIN ${lib.escapeShellArg tyBin} \
        --set IX_MCP_TY_PYTHON ${lib.escapeShellArg mcpPython.interpreter} \
        --set IX_NIX_WEB_MONITOR_BIN ${lib.escapeShellArg (lib.getExe nixWebMonitorBin)} \
        --set-default SSL_CERT_FILE ${lib.escapeShellArg caBundle} \
        --set-default CURL_CA_BUNDLE ${lib.escapeShellArg caBundle} \
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
        ${fabricWrapperFlags} \
        --set PLAYWRIGHT_BROWSERS_PATH ${lib.escapeShellArg playwrightBrowsers} \
        --set IX_SVELTE_BUNDLE_BIN ${lib.escapeShellArg (lib.getExe svelteBundleBin)} \
        --set IX_GCAL_BIN ${lib.escapeShellArg "${gcalBin}/bin/gcal"} \
        --set SCIPQL_SOUFFLE ${lib.escapeShellArg (lib.getExe' pkgs.souffle "souffle")} \
        --set IX_MCP_TY_BIN ${lib.escapeShellArg tyBin} \
        --set IX_MCP_TY_PYTHON ${lib.escapeShellArg mcpPython.interpreter} \
        --set IX_NIX_WEB_MONITOR_BIN ${lib.escapeShellArg (lib.getExe nixWebMonitorBin)} \
        --set-default SSL_CERT_FILE ${lib.escapeShellArg caBundle} \
        --set-default CURL_CA_BUNDLE ${lib.escapeShellArg caBundle} \
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
  # importable on the same interpreter and third-party base the kernels run on,
  # which is a plain interpreter import (no kernel, no network), so the build
  # sandbox can prove it. `modules` names the first-party modules under test;
  # the env carries the shared third-party base plus their declared-dep closure
  # only, so editing an unrelated module does not re-run this test.
  importTest = modules: name: code: let
    testPython = bundledTestPython modules;
  in
    pkgs.runCommand "ix-mcp-${name}"
    {
      nativeBuildInputs = [testPython];
      strictDeps = true;
    }
    ''
      ${lib.getExe testPython} -c ${lib.escapeShellArg code} >stdout 2>stderr || {
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
  # A third-party-only interpreter is passed as `--python-executable` so every
  # bundled PyPI/nixpkgs dependency (polars, mcp, jupyter, ...) resolves exactly
  # as it does at runtime, while the first-party sources resolve from the check
  # tree assembled below.
  #
  # Scoped, not all-or-nothing: only the modules under `strictGreenModules` are
  # checked, so a module is migrated by adding it here once its source is fully
  # annotated and clean. The check tree carries the green modules plus their
  # transitive first-party imports (`ixFirstPartyDeps`, the same declarations
  # the per-module test envs use), so a checked module's cross-imports (e.g.
  # `x` -> `browser`, `nox_autotriage` -> `linear`) resolve even before those
  # deps are migrated -- and editing a module outside that closure cannot
  # invalidate the gate. The `ix_notebook_mcp` server package and the remaining
  # `src/*` modules are added here as they are brought up to strict.
  strictGreenModules = map bundledModule [
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
    "fabric"
    "claude_history"
    "embed"
  ];
  # The `ix_notebook_mcp` server package is migrated file-by-file (the package
  # as a whole is still ~200 errors from strict-clean, index#1902): each file
  # here is a check target inside the copied `ix_notebook_mcp/` tree. zuban
  # only reports errors in the named targets, so a listed file's imports of
  # still-unmigrated siblings do not drag their errors in.
  strictGreenServerFiles = [
    "tools.py"
    "mcp_ui.py"
  ];
  zubanConfig = (pkgs.formats.ini {}).generate "ix-mcp-zuban.ini" {
    mypy = {};
    # Pygments builds public re-exports through module __getattr__, and several
    # lexer/highlight helpers remain untyped in its partial stubs.
    "mypy-pygments.*".disallow_untyped_calls = false;
  };
  # Third-party deps only: the first-party sources come from the check tree, so
  # editing a module outside the green closure leaves this env untouched.
  strictTypecheckPython = mcpPythonInterp.withPackages mcpTestBasePackages;
  strictTypecheck = let
    # The check tree: each bundled module's build output IS its source in
    # site-packages layout, so the tree is assembled from the green modules
    # plus their declared-dep closure and nothing else.
    checkTree = firstPartyClosure strictGreenModules;
    sitePackagesOf = modules: map (m: "${m}/${pkgs.python3.sitePackages}") modules;
  in
    pkgs.runCommand "ix-mcp-strict-typecheck"
    {
      nativeBuildInputs = [
        pkgs.zuban
        pkgs.ruff
        strictTypecheckPython
      ];
      strictDeps = true;
      meta.description = "zuban --strict + ruff ANN over the migrated ix-mcp Python sources";
    }
    ''
      mkdir checkroot
      closure=(${lib.escapeShellArgs (sitePackagesOf checkTree)})
      for tree in "''${closure[@]}"; do
        cp -r "$tree"/. checkroot/
      done
      chmod -R u+w checkroot
      cp ${zubanConfig} zuban.ini

      # Targets are the green modules' package dirs (one site-packages entry
      # per module output) plus the migrated server files.
      targets=()
      green=(${lib.escapeShellArgs (sitePackagesOf strictGreenModules)})
      for tree in "''${green[@]}"; do
        for pkg in "$tree"/*; do
          targets+=("checkroot/$(basename "$pkg")")
        done
      done
      serverfiles=(${lib.escapeShellArgs strictGreenServerFiles})
      for serverfile in "''${serverfiles[@]}"; do
        targets+=("checkroot/ix_notebook_mcp/$serverfile")
      done

      export MYPYPATH=checkroot:.
      echo "zuban check --strict over: ''${targets[*]}"
      zuban check --strict \
        --config-file zuban.ini \
        --python-executable ${strictTypecheckPython.interpreter} \
        --python-version ${pkgs.python3.pythonVersion} \
        --platform linux \
        "''${targets[@]}"
      echo "ruff check (ANN + TID251 no-cast) over: ''${targets[*]}"
      ruff check ${ix.ruffAnnArgs} "''${targets[@]}"

      mkdir -p "$out"
    '';

  tuiBundled = importTest [tuiModule] "tui" "import tui; print('tui-ok', tui.__version__)";
  # htpy must import and auto-escape: a `<` in a text node comes out as `&lt;`.
  htpyBundled = importTest [] "htpy" "import htpy; print('htpy-ok' if '&lt;' in str(htpy.div['<']) else 'htpy-bad')";
  searchBundled = importTest [searchModule] "search" "import search; print('search-ok', search.__version__)";
  # The astlog surface is callable and its public API returns polars frames:
  # `scan`/`fixes`/`suppressed` are DataFrames and `query` a dict of them. A
  # trivial inline rules string against one temp file keeps it fast and offline;
  # an `astlog-ignore` comment exercises the suppression-listing path end to end.
  astlogBundled = importTest [astlogModule] "astlog" ''
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

  dataLibsBundled = importTest [] "data-libs" (
    "import psycopg, sqlalchemy, duckdb, httpx; "
    + "from sqlalchemy import create_engine; create_engine('postgresql+psycopg://u@h/db'); "
    + "from pypdf import PdfReader; "
    + "print('data-libs-ok')"
  );
  gmailLibsBundled = importTest [] "gmail-libs" (
    "from googleapiclient.discovery import build; from google.oauth2.credentials import Credentials; "
    + "import google_auth_oauthlib, google_auth_httplib2; "
    + "build('gmail', 'v1', credentials=Credentials(token='x'), static_discovery=True); "
    + "print('gmail-libs-ok')"
  );
  cursorSdkBundled = importTest [] "cursor-sdk" (
    "import cursor_sdk; from cursor_sdk import AsyncAgent, AsyncClient; "
    + "assert callable(getattr(AsyncAgent, 'create', None)); "
    + "print('cursor-sdk-ok')"
  );
  exaBundled = importTest [] "exa" (
    "from exa_py import Exa; e = Exa('dummy-key'); "
    + "assert callable(e.search) and callable(e.answer); "
    + "print('exa-ok')"
  );
  # Typed PyO3 bindings: the cdylib loads and the two Client classes are
  # callable. A real call would need GOOGLE_OAUTH_CLIENT_ID/SECRET and a
  # token file, so the sandbox-safe assertion is the import and the
  # class-shape check.
  ixGoogleBundled = importTest [ixGoogleModule] "ix-google" (
    "import ix_google; from ix_google import gmail, calendar; "
    + "assert callable(gmail.Client) and callable(calendar.Client); "
    + "print('ix-google-ok', ix_google.__version__)"
  );
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
  engineBundled = importTest [] "engine" "import ipykernel, jupyter_client, nbformat, aiohttp, mcp; print('engine-ok')";

  # The server package imports and registers its full tool surface. Exercises the
  # FastMCP registration (schemas from type hints) without starting a kernel or
  # the Jupyter Server, so it is sandbox-safe.
  # Every first-party `src/` package (each one becomes a toPythonModule above)
  # must surface in the `api()` catalog (a `registry.MODULES` row) or carry an
  # explicit reason in `registry.UNCATALOGED`: `svelte` was bundled but missing
  # from the catalog for months (index#3091). Derived from the directory
  # listing so the next module cannot repeat that; the filter drops the lone
  # `private_session.py` file (a shared guard, not a module dir).
  srcModules = builtins.attrNames (
    lib.filterAttrs (_: type: type == "directory") (builtins.readDir ./src)
  );
  serverTools = importTest [ixNotebookMcpModule] "server" (
    "import asyncio; from ix_notebook_mcp.tools import mcp; "
    + "names = sorted(t.name for t in asyncio.run(mcp.list_tools())); "
    # This set drifts silently: session_set_name (#1615) and kernel_restart
    # (#2349) each joined the surface without updating it, dropping the read
    # tool (#3503) left it listed here, and each time the stale drv kept
    # passing from cache on main until this package's inputs changed and
    # forced a rebuild. When adding or removing a tool, update it in the same
    # change.
    + "expected = {'python_exec','pr_watch','kernel_trace','kernel_restart','tui_act','session_set_name','topic_set','reply'}; "
    + "assert set(names) == expected, ('tool surface drifted: %r' % (names,)); "
    + "from ix_notebook_mcp import registry; instr = mcp._mcp_server.instructions; "
    + "assert 'root=' not in instr, 'a parameter/signature leaked into the instructions'; "
    + "assert '(query:' not in instr and '(path:' not in instr, 'a signature leaked into the instructions'; "
    + "missing = [m.name for m in registry.MODULES if ('`' + m.name + '`') not in instr]; "
    + "assert not missing, ('registry modules missing from instructions: %r' % (missing,)); "
    + "import json; bundled = json.loads(${builtins.toJSON (builtins.toJSON srcModules)}); "
    + "cataloged = set(registry.module_names()) | set(registry.UNCATALOGED); "
    + "dropped = [n for n in bundled if n not in cataloged]; "
    + "assert not dropped, ('bundled src/ modules missing from the api() catalog -- add a registry.Module row or a registry.UNCATALOGED reason: %r' % (dropped,)); "
    + "stale = sorted(set(registry.UNCATALOGED) - set(bundled)); "
    + "assert not stale, ('registry.UNCATALOGED names modules not under src/: %r' % (stale,)); "
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

  # Issue #1754: per-cell static type checking (ty) before execution, plus the
  # bug 1-3 regressions (await-a-failed-job re-raises; Job/Result accessor
  # symmetry; fsearch partial-on-timeout + limit short-circuit). The type-check
  # tests need ty resolvable and its diagnostics stable, so ty is provided on the
  # env exactly as the wrapper sets it; rg/fd back the fsearch limit assertion.
  # A dedicated interpreter adds pytest (the bare test envs omit it). The
  # battery imports ix_notebook_mcp, fsearch, sh, nu, weave (all in the server
  # closure) plus claude_history.
  typecheckTestPython = bundledTestPythonWith (ps: [ps.pytest]) [
    ixNotebookMcpModule
    claudeHistoryModule
  ];
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
      meta.description = "per-cell type check (ty) + issue #1754 bug 1-3 regressions + sh exit surfacing (#1766) + Result.value reachability (#2068) + find glob= filter (#1366) + in-band build stamp (#2110) + session-scoped job cancellation (#2104) + client-cancel interrupts in-flight run (#2387) + jobs.spawn ad-hoc awaitables (#2164) + grep files_only (#2246) + claude-history session search (#2245) + per-serve kernel trace file (#2355) + builtin shadow restore (#2430) + failed-cell stale-binding note (#2526) + pr_watch instant-merge guard (#2532) + find glob-pattern autodetect (#2542) + nu input= routing past no-input statements (#2540) + read target top-level await (#3139) + nu-job line paging (#3131) + kernel host seam: local child vs ray actor";
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      export IX_MCP_TY_BIN=${lib.escapeShellArg tyBin}
      export IX_MCP_TY_PYTHON=${lib.escapeShellArg typecheckTestPython.interpreter}
      # The edited ix_notebook_mcp / fsearch / sh live in the interpreter's
      # site-packages (built from this worktree's source), so the tests import
      # them from there; only the test files are copied in (a bare store path of
      # a single .py is read by pytest as a directory).
      cp ${./tests/test_typecheck.py} test_typecheck.py
      cp ${./tests/test_job_await_errors.py} test_job_await_errors.py
      # Issue #2104: one session's wait must never cancel another session's job.
      cp ${./tests/test_job_cancel_scope.py} test_job_cancel_scope.py
      # Issue #2387: a client that cancels an in-flight python_exec cancels the
      # backgrounded run it launched, instead of executing side effects after.
      cp ${./tests/test_cancel_running.py} test_cancel_running.py
      # Issue #2164: jobs.spawn registers an ad-hoc awaitable as a first-class job.
      cp ${./tests/test_jobs_spawn.py} test_jobs_spawn.py
      # A spawned job starts and finishes the same proc entity; no detached run phantom.
      cp ${./tests/test_spawn_store_lifecycle.py} test_spawn_store_lifecycle.py
      cp ${./tests/test_fsearch_partial.py} test_fsearch_partial.py
      cp ${./tests/test_fsearch_glob.py} test_fsearch_glob.py
      # Issue #2542: find('*.py') auto-detects a glob-shaped non-regex pattern.
      cp ${./tests/test_fsearch_glob_pattern.py} test_fsearch_glob_pattern.py
      # Issue #2246: grep(files_only=True) -> path + match-count rows via rg --count-matches.
      cp ${./tests/test_fsearch_files_only.py} test_fsearch_files_only.py
      # Issue #2245: ranked per-session search over local Claude Code history.
      cp ${./tests/test_claude_history.py} test_claude_history.py
      # sh Output rendering regressions (issue #1766: a failed build must not
      # read as success/still-running); imports the site-packages sh module.
      cp ${./tests/test_sh_module.py} test_sh_module.py
      # Issue #2355: per-serve kernel trace file + sweep of orphaned dumps.
      cp ${./tests/test_kernel_trace_path.py} test_kernel_trace_path.py
      # The kernel host seam: local/ray selection, the actor's connection-info
      # plumbing (str HMAC key), offset-scoped trace reads.
      cp ${./tests/test_kernel_host.py} test_kernel_host.py
      # The kernel's board lease: registration placement facts (kernel_host,
      # node) and the writer's heartbeat_ms beat, agent idle-clock untouched.
      cp ${./tests/test_store_kernel_lease.py} test_store_kernel_lease.py
      # Issue #2430: a cell rebinding/deleting a kernel builtin gets it restored.
      cp ${./tests/test_builtin_shadow_restore.py} test_builtin_shadow_restore.py
      # Issue #2526: a failed cell's traceback names the bindings it never reached.
      cp ${./tests/test_unexecuted_note.py} test_unexecuted_note.py
      # Issue #2532: watch_pr skips arming auto merge on an already-mergeable PR.
      cp ${./tests/test_pr_watch_automerge.py} test_pr_watch_automerge.py
      # Issue #2540: input= routes past no-input statements (cd /tmp; ^cat) or raises.
      cp ${./tests/test_nu_input_routing.py} test_nu_input_routing.py
      # Issue #3131: a job wrapping nu(check=False) pages real stdout lines.
      cp ${./tests/test_nu_job_output.py} test_nu_job_output.py
      # Durable-local-first store writes (index#3418/#3419): outage-durable
      # spool, append-order drain, one loud line, no wire wait on the caller.
      cp ${./tests/weave_stub.py} weave_stub.py
      cp ${./tests/test_store_spool.py} test_store_spool.py
      ${lib.getExe typecheckTestPython} -m pytest \
        test_typecheck.py test_job_await_errors.py test_job_cancel_scope.py \
        test_cancel_running.py \
        test_jobs_spawn.py \
        test_spawn_store_lifecycle.py \
        test_fsearch_partial.py \
        test_fsearch_glob.py \
        test_fsearch_glob_pattern.py \
        test_fsearch_files_only.py \
        test_claude_history.py \
        test_sh_module.py \
        test_kernel_trace_path.py \
        test_kernel_host.py \
        test_store_kernel_lease.py \
        test_builtin_shadow_restore.py \
        test_unexecuted_note.py \
        test_pr_watch_automerge.py \
        test_nu_input_routing.py \
        test_nu_job_output.py \
        test_store_spool.py \
        -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp typecheck smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';

  coreLocationBundled = importTest [] "corelocation" "import CoreLocation; print('corelocation-ok', callable(CoreLocation.CLLocationManager.alloc))";
  scriptingBridgeBundled = importTest [] "scriptingbridge" "import ScriptingBridge; print('scriptingbridge-ok', callable(ScriptingBridge.SBApplication.applicationWithBundleIdentifier_))";
  nuBundled = importTest [nuPyModule] "nu" "import nu; print('nu-ok', callable(nu), callable(nu.value), nu.NuError.__name__ == 'NuError', nu.__version__)";
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
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        inherit embedCli;
        tests =
          {
            inherit
              strictTypecheck
              tuiBundled
              htpyBundled
              searchBundled
              astlogBundled
              dataLibsBundled
              gmailLibsBundled
              exaBundled
              cursorSdkBundled
              ixGoogleBundled
              nuBundled
              nuTests
              requirementsSmoke
              engineBundled
              serverTools
              evalSmoke
              typecheckSmoke
              ;
          }
          // lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
            inherit
              coreLocationBundled
              scriptingBridgeBundled
              ;
          }
          # Every discovered module dir's own tests (module.nix `tests`,
          # darwin-gated inside the modules that are darwin-only).
          // bundledModuleTests;
      }
      // lib.optionalAttrs (updateScript != null) {
        inherit updateScript;
      };
  })
