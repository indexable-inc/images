{
  ix,
  lib,
  pkgs,
  ...
}:

let
  meta = {
    description = "Record a terminal demo reel by driving real CLIs through the tui PTY driver, rasterizing the styled grid to an animated WebP";
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
  # bash is the driven shell, and git/python3 are the demoed programs. They must
  # be on PATH at runtime, so the bare binary is wrapped rather than exposed raw.
  runtimeInputs = [
    pkgs.ffmpeg
    pkgs.bashInteractive
    pkgs.git
    pkgs.python3
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
