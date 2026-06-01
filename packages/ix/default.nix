{
  lib,
  stdenvNoCC,
  fetchurl,
  darwin,
  cliArtifacts ? { },
  packageSystem ? stdenvNoCC.hostPlatform.system,
  binarySrc ? null,
}:

let
  sources = {
    x86_64-linux = fetchurl {
      url = "https://ix.dev/cli/linux-x86_64/ix";
      hash = "sha256-LRbnBfNRFmLV2o25V03HXV7vCIS6C8fhMSTypKnLMw8=";
    };
    aarch64-darwin = fetchurl {
      url = "https://ix.dev/cli/darwin-arm64/ix";
      hash = "sha256-HNt7tsElD0lc+Y+fNTNWw8vLdR1flsK1lnsyjhG6hAg=";
    };
    x86_64-darwin = fetchurl {
      url = "https://ix.dev/cli/darwin-x86_64/ix";
      hash = "sha256-aNjAKbnqitcj7HU9XGuFtBe2aucekE4CVwdPCuxB22k=";
    };
  };

  inherit (stdenvNoCC.hostPlatform) system isDarwin;
  artifactSrc = cliArtifacts.${packageSystem} or null;
  selectedSrc =
    if binarySrc != null then
      binarySrc
    else if artifactSrc != null then
      artifactSrc
    else
      sources.${system} or (throw "ix CLI: no precompiled binary for ${system}");
in
stdenvNoCC.mkDerivation {
  pname = "ix";
  version = "precompiled";

  src = selectedSrc;

  dontUnpack = true;
  dontBuild = true;
  strictDeps = true;

  # The darwin binaries ix.dev serves carry an invalid code signature
  # (`codesign --verify` reports "code or signature have been modified"), so
  # macOS AMFI kills them with SIGKILL the moment they exec and `nix run .#ix`
  # never prints a line. Re-sign ad-hoc at install time to make the CLI launch.
  nativeBuildInputs = lib.optionals isDarwin [
    darwin.sigtool
    darwin.cctools
  ];

  installPhase = ''
    runHook preInstall

    install -Dm755 "$src" "$out/bin/ix"

    runHook postInstall
  '';

  postInstall = lib.optionalString isDarwin ''
    codesign --force --sign - "$out/bin/ix"
  '';

  meta = {
    description = "ix deployment platform CLI";
    mainProgram = "ix";
    platforms = builtins.attrNames sources;
  };
}
