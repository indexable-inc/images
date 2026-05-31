{
  ix,
  lib,
  pkgs ? ix.pkgs,
}:

let
  unwrapped = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "ix-mcp";
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
  # `--system-site-packages`, so `tui`, `search`, numpy, polars, and
  # playwright are importable by default while an in-session `pip install` still
  # writes to the per-session venv.
  mcpPython = pkgs.python3.withPackages (
    ps:
    [
      ps.asyncssh
      ps.numpy
      ps.polars
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
      tuiModule
      searchModule
    ]
    ++ darwinExtraPackages ps
  );

  # Browser bundle that matches the playwright-driver the python package is
  # patched to use. Exposed to the worker through PLAYWRIGHT_BROWSERS_PATH on the
  # wrapper below so launched browsers resolve without a network download.
  playwrightBrowsers = pkgs.playwright-driver.browsers;

  package =
    pkgs.runCommand "ix-mcp"
      {
        nativeBuildInputs = [ pkgs.makeWrapper ];
        strictDeps = true;
        meta = (unwrapped.meta or { }) // {
          mainProgram = "ix-mcp";
        };
      }
      ''
        mkdir -p $out/bin
        makeWrapper ${unwrapped}/bin/ix-mcp $out/bin/ix-mcp \
          --set IX_MCP_PYTHON ${lib.escapeShellArg (lib.getExe mcpPython)} \
          --set PLAYWRIGHT_BROWSERS_PATH ${lib.escapeShellArg playwrightBrowsers} \
          ${lib.optionalString pkgs.stdenv.hostPlatform.isDarwin "--set IX_MACVM_BIN ${lib.escapeShellArg "${macosVmBin}/bin/macos-vm"}"}
      '';
  replDefault =
    pkgs.runCommand "ix-mcp-repl-default"
      {
        nativeBuildInputs = [ package ];
        strictDeps = true;
      }
      ''
                export HOME=$TMPDIR/home
                mkdir -p "$HOME"

        	        printf 'print("ix-mcp-repl-ok")\nraise SystemExit(0)\n' | ix-mcp repl >stdout 2>stderr
        	        grep -q '^ix-mcp-repl-ok$' stdout
        	        if find "$TMPDIR" -maxdepth 1 -type d -name 'ix-mcp-python-repl-*' | grep -q .; then
        	          echo "default REPL temp directory leaked" >&2
        	          exit 1
        	        fi

        	        mkdir -p "$out"
        	      '';
  sessionVenv =
    pkgs.runCommand "ix-mcp-session-venv"
      {
        nativeBuildInputs = [ package ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"

        # A session must activate its venv for child processes so an in-session
        # `pip` resolves the per-session venv rather than the host. Without
        # activation, `shutil.which("pip")` misses the venv bin on PATH and
        # returns the host pip or None.
        ix-mcp eval '__import__("shutil").which("pip")' >stdout 2>stderr
        if ! grep -q '/[.]venv/bin/pip' stdout; then
          echo "in-session pip did not resolve to the venv:" >&2
          cat stdout >&2
          exit 1
        fi

        mkdir -p "$out"
      '';
  tuiBundled =
    pkgs.runCommand "ix-mcp-tui-bundled"
      {
        nativeBuildInputs = [ package ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"

        # `tui` ships in the pinned interpreter, so a bare session imports it
        # with no install step. Importing loads the PyO3 cdylib, which exercises
        # the link (the macOS dynamic_lookup path in particular); spawning a real
        # PTY needs device nodes the build sandbox lacks, so leave that to
        # runtime.
        ix-mcp exec 'import tui; print("tui-ok", tui.__version__)' >stdout 2>stderr
        if ! grep -q '^tui-ok ' stdout; then
          echo "tui was not importable in a default session:" >&2
          cat stdout stderr >&2
          exit 1
        fi

        mkdir -p "$out"
      '';
  searchBundled =
    pkgs.runCommand "ix-mcp-search-bundled"
      {
        nativeBuildInputs = [ package ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"

        # `search` ships in the pinned interpreter, so a bare session
        # imports it with no install step. Importing loads the PyO3 cdylib, which
        # exercises the link (the macOS dynamic_lookup path in particular).
        # Running a real search needs network and a credential the build sandbox
        # lacks, so leave that to runtime.
        ix-mcp exec 'import search; print("search-ok", search.__version__)' >stdout 2>stderr
        if ! grep -q '^search-ok ' stdout; then
          echo "search was not importable in a default session:" >&2
          cat stdout stderr >&2
          exit 1
        fi

        mkdir -p "$out"
      '';
  # macOS-only: `screen` ships in the pinned interpreter there, so a bare
  # session imports it with no install step. Importing exercises the pyobjc
  # `Quartz` link and the ctypes load of ApplicationServices for the TCC probe.
  # A real capture or synthetic input needs a display and Accessibility grant
  # the build sandbox lacks, so leave those to runtime; the import plus the
  # callable surface is what this guards.
  screenBundled =
    pkgs.runCommand "ix-mcp-screen-bundled"
      {
        nativeBuildInputs = [ package ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"

        ix-mcp exec 'import screen; print("screen-ok", callable(screen.capture), callable(screen.click), callable(screen.accessibility_trusted))' >stdout 2>stderr
        if ! grep -q '^screen-ok True True True$' stdout; then
          echo "screen was not importable in a default session:" >&2
          cat stdout stderr >&2
          exit 1
        fi

        mkdir -p "$out"
      '';
  # macOS-only: `macvm` ships in the pinned interpreter there. Importing checks
  # the module loads and its callable surface; a real boot needs Apple's
  # Virtualization.framework + a guest bundle the build sandbox lacks, so leave
  # that to runtime.
  macvmBundled =
    pkgs.runCommand "ix-mcp-macvm-bundled"
      {
        nativeBuildInputs = [ package ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"

        ix-mcp exec 'import macvm; print("macvm-ok", *(callable(getattr(macvm, n)) for n in ("info", "install", "screenshot", "screenshot_many", "drive", "Driver")))' >stdout 2>stderr
        if ! grep -q '^macvm-ok True True True True True True$' stdout; then
          echo "macvm was not importable in a default session:" >&2
          cat stdout stderr >&2
          exit 1
        fi

        mkdir -p "$out"
      '';
  sessionSubprocessStdin =
    pkgs.runCommand "ix-mcp-session-subprocess-stdin"
      {
        nativeBuildInputs = [ package ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        mkdir -p "$HOME"

        # A child spawned inside a session inherits fd 0. The worker speaks
        # JSON-RPC over its own stdin, so an inherited fd 0 pointing at that pipe
        # lets a stdin-reading child (a path-less `cat`/`rg`) steal the protocol
        # channel and hang the session forever. The worker detaches fd 0 to
        # /dev/null, so `cat` with no path reads EOF and returns at once. The
        # timeout turns a regression into a failure instead of a hung build.
        timeout 60 ix-mcp exec 'import subprocess; print("cat-rc", subprocess.run(["cat"], capture_output=True, text=True).returncode)' >stdout 2>stderr
        if ! grep -q '^cat-rc 0$' stdout; then
          echo "session subprocess inherited the RPC stdin (hang regression):" >&2
          cat stdout stderr >&2
          exit 1
        fi

        mkdir -p "$out"
      '';
in
package.overrideAttrs (old: {
  passthru =
    (old.passthru or { })
    // unwrapped.passthru
    // {
      inherit unwrapped;
      tests =
        (unwrapped.passthru.tests or { })
        // {
          inherit
            replDefault
            sessionVenv
            tuiBundled
            searchBundled
            sessionSubprocessStdin
            ;
        }
        # `screen` is only bundled on Darwin, so its import test only exists there.
        // lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
          inherit screenBundled macvmBundled;
        };
    };
})
