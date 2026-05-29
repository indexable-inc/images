{
  ix,
  lib,
  pkgs ? ix.pkgs,
}:

let
  unwrapped = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "ix-mcp";
  };
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
          --set IX_MCP_PYTHON ${lib.escapeShellArg (lib.getExe pkgs.python3)}
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
in
package.overrideAttrs (old: {
  passthru =
    (old.passthru or { })
    // unwrapped.passthru
    // {
      inherit unwrapped;
      tests = (unwrapped.passthru.tests or { }) // {
        inherit replDefault;
      };
    };
})
