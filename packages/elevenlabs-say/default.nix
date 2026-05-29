{
  ix,
  lib,
  pkgs,
}:

let
  fs = lib.fileset;
  src = fs.toSource {
    root = ./.;
    fileset = fs.unions [
      ./pyproject.toml
      ./src
      ./uv.lock
    ];
  };

  unwrapped = ix.buildUvApplication pkgs {
    pname = "elevenlabs-say";
    version = "0.1.0";
    inherit src;
    mainProgram = "elevenlabs-say";
    # pydantic-core and websockets ship binary wheels that dlopen libstdc++ at
    # import time on Linux, the same constraint the daily-scraper example handles.
    runtimeLibraryInputs = [ pkgs.stdenv.cc.cc.lib ];
    meta = {
      description = "A say-style ElevenLabs text-to-speech CLI";
      license = lib.licenses.mit;
      mainProgram = "elevenlabs-say";
    };
  };

  package =
    pkgs.runCommand "elevenlabs-say"
      {
        nativeBuildInputs = [ pkgs.makeWrapper ];
        strictDeps = true;
        meta = {
          description = "A say-style ElevenLabs text-to-speech CLI";
          license = lib.licenses.mit;
          mainProgram = "elevenlabs-say";
        };
      }
      ''
        mkdir -p $out/bin
        # ffmpeg supplies ffplay, which playback shells out to. afplay is
        # macOS-only and absent from nixpkgs, so ffplay is the portable choice.
        makeWrapper ${lib.getExe unwrapped} $out/bin/elevenlabs-say \
          --prefix PATH : ${lib.makeBinPath [ pkgs.ffmpeg ]}
      '';

  printsHelp =
    pkgs.runCommand "elevenlabs-say-prints-help"
      {
        nativeBuildInputs = [ package ];
        strictDeps = true;
      }
      ''
        # No network and no API key: --help must exit 0 and print usage.
        help=$(elevenlabs-say --help)
        case "$help" in
          *"usage: elevenlabs-say"*) ;;
          *)
            echo "elevenlabs-say --help did not print usage" >&2
            printf '%s\n' "$help" >&2
            exit 1
            ;;
        esac
        mkdir -p "$out"
      '';
in
package.overrideAttrs (old: {
  passthru = (old.passthru or { }) // {
    tests = {
      inherit printsHelp;
    };
  };
})
