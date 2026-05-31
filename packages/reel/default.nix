{
  ix,
  lib,
  pkgs,
  ...
}:

let
  # The repo tools the demo runs, built from the shared workspace unit graph for
  # this pkgs (the same way their own packages build them), so the wrapper gets
  # host-correct binaries without depending on the repo overlay.
  fileSearch = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "file-search";
    meta = {
      description = "BM25 file indexer and searcher built on Tantivy";
      license = lib.licenses.mit;
      mainProgram = "file-search";
    };
  };
  gitLogPretty = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "git-log-pretty";
    meta = {
      description = "Pretty git log viewer with file-icon trees";
      license = lib.licenses.mit;
      mainProgram = "git-log-pretty";
    };
  };

  meta = {
    description = "Record a terminal demo reel by driving real CLIs through the tui PTY driver, rasterizing the styled grid to an animated AVIF (with a WebP fallback)";
    # The crate is MIT (repo LICENSE); the binary embeds JetBrains Mono, which
    # is SIL Open Font License 1.1 (see packages/reel/fonts/OFL.txt).
    license = [
      lib.licenses.mit
      lib.licenses.ofl
    ];
    mainProgram = "reel";
  };

  unwrapped = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "reel";
    inherit meta;
  };

  # reel shells out to these by name while recording: ffmpeg encodes the frames,
  # bash is the driven shell, and the rest are the demoed programs (the repo's
  # own file-search and pretty-log, git for history, python for the PTY-driver
  # scene). They must be on PATH at runtime, so the bare binary is wrapped rather
  # than exposed raw.
  runtimeInputs = [
    pkgs.ffmpeg
    pkgs.bashInteractive
    pkgs.git
    pkgs.python3
    fileSearch
    gitLogPretty
  ];

  wrapped =
    pkgs.runCommand "reel"
      {
        nativeBuildInputs = [ pkgs.makeWrapper ];
        strictDeps = true;
        inherit meta;
      }
      ''
        mkdir -p $out/bin
        makeWrapper ${lib.getExe unwrapped} $out/bin/reel \
          --prefix PATH : ${lib.makeBinPath runtimeInputs}
      '';

  printsHelp =
    pkgs.runCommand "reel-prints-help"
      {
        nativeBuildInputs = [ wrapped ];
        strictDeps = true;
      }
      ''
        # No display, no scenes recorded: --help must exit 0 and print usage.
        help=$(reel --help)
        case "$help" in
          *"Usage: reel"*) ;;
          *)
            echo "reel --help did not print usage" >&2
            printf '%s\n' "$help" >&2
            exit 1
            ;;
        esac
        mkdir -p "$out"
      '';
in
wrapped.overrideAttrs (old: {
  passthru = (old.passthru or { }) // {
    tests = (unwrapped.passthru.tests or { }) // {
      inherit printsHelp;
    };
  };
})
