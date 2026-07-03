{
  ix,
  lib,
  pkgs,
}: let
  package = ix.buildUvApplication pkgs {
    pname = "mlx-tts";
    version = "0.1.0";
    srcRoot = ./.;
    python = pkgs.python312;
    mainProgram = "mlx-tts";
    pythonPlatform = "darwin";
    runtimeLibraryInputs = [pkgs.stdenv.cc.cc.lib];
    meta = {
      description = "Quality-first local Apple Silicon text-to-speech through MLX-Audio";
      license = lib.licenses.mit;
      mainProgram = "mlx-tts";
      platforms = ["aarch64-darwin"];
    };
  };

  printsHelp =
    pkgs.runCommand "mlx-tts-prints-help"
    {
      nativeBuildInputs = [package];
      strictDeps = true;
    }
    ''
          mlx-tts --help | grep -q 'usage: mlx-tts'
      mkdir -p "$out"
    '';
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests = {
          inherit printsHelp;
        };
      };
  })
