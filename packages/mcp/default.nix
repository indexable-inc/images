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

  # The interpreter the wrapper pins. Sessions build their venv from this with
  # `--system-site-packages`, so `tui` and numpy are importable by default while
  # an in-session `pip install` still writes to the per-session venv.
  mcpPython = pkgs.python3.withPackages (ps: [
    ps.asyncssh
    ps.numpy
    tuiModule
  ]);

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
          --set IX_MCP_PYTHON ${lib.escapeShellArg (lib.getExe mcpPython)}
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
in
package.overrideAttrs (old: {
  passthru =
    (old.passthru or { })
    // unwrapped.passthru
    // {
      inherit unwrapped;
      tests = (unwrapped.passthru.tests or { }) // {
        inherit replDefault sessionVenv tuiBundled;
      };
    };
})
