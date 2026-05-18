{
  stdenvNoCC,
  fetchurl,
}:

let
  sources = {
    x86_64-linux = fetchurl {
      url = "https://ix.dev/cli/linux-x86_64/ix";
      hash = "sha256-eXdV+aSEQNIssqfRywiIOk+979WkOLJapa7C2D7C6Ts=";
    };
    aarch64-darwin = fetchurl {
      url = "https://ix.dev/cli/darwin-arm64/ix";
      hash = "sha256-b6wwHtikhMYl/FfpmepU3ONOR+Wtcm5HE5zQIK7qC6M=";
    };
    x86_64-darwin = fetchurl {
      url = "https://ix.dev/cli/darwin-x86_64/ix";
      hash = "sha256-LATWkElzdoSxO95f3n1hOwauMBuLkLSsyIDdccbmhJ4=";
    };
  };

  inherit (stdenvNoCC.hostPlatform) system;
in
stdenvNoCC.mkDerivation {
  pname = "ix";
  version = "precompiled";

  src = sources.${system} or (throw "ix CLI: no precompiled binary for ${system}");

  dontUnpack = true;
  dontBuild = true;
  strictDeps = true;

  installPhase = ''
    runHook preInstall

    install -Dm755 "$src" "$out/bin/ix"

    runHook postInstall
  '';

  meta = {
    description = "ix deployment platform CLI";
    mainProgram = "ix";
    platforms = builtins.attrNames sources;
  };
}
