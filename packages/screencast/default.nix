{
  ix,
  lib,
  pkgs,
  ...
}:

let
  meta = {
    description = "Stream the macOS desktop to a screencast-ingest server as hardware-encoded H.265 (VideoToolbox) over fragmented-MP4 HLS";
    license = lib.licenses.mit;
    mainProgram = "screencast";
    # avfoundation capture and hevc_videotoolbox are macOS-only.
    platforms = lib.platforms.darwin;
  };

  unwrapped = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "screencast";
    inherit meta;
  };

  # screencast shells out to ffmpeg to capture, encode, and upload, so ffmpeg
  # must be on PATH at runtime. nixpkgs ffmpeg is built with VideoToolbox on
  # darwin, so it carries the required hevc_videotoolbox encoder.
  wrapped =
    pkgs.runCommand "screencast"
      {
        nativeBuildInputs = [ pkgs.makeWrapper ];
        strictDeps = true;
        inherit meta;
      }
      ''
        mkdir -p $out/bin
        makeWrapper ${lib.getExe unwrapped} $out/bin/screencast \
          --prefix PATH : ${lib.makeBinPath [ pkgs.ffmpeg ]}
      '';

  printsHelp =
    pkgs.runCommand "screencast-prints-help"
      {
        nativeBuildInputs = [ wrapped ];
        strictDeps = true;
      }
      ''
        help=$(screencast --help)
        case "$help" in
          *"Usage: screencast"*) ;;
          *)
            echo "screencast --help did not print usage" >&2
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
