# omp (oh-my-pi) coding agent, packaged from the upstream GitHub release
# binaries. Upstream ships self-contained `bun build --compile` executables and
# CI-verifies exactly these artifacts; a from-source build under nix needs
# network inside the sandbox at three stages (bun dependency install, the
# cross-target bun runtime download, the napi addon pipeline), so the release
# asset is the packaging boundary. Bumping: update `version`, refresh each
# hash from the per-asset sha256 digests on the release page
# (`gh release view v<version> --repo can1357/oh-my-pi --json assets`), then
# run smoke/claude-hooks-smoke.sh to catch extension-API drift in the
# Claude bridges (extensions/claude-hooks.ts, claude-agents.ts).
#
# Runtime notes: the binary extracts its native addon to ~/.omp/natives/<v>/ on
# first run and self-update (`omp update`) fails harmlessly against the
# read-only store — update by bumping this file instead.
{
  lib,
  stdenvNoCC,
  fetchurl,
  autoPatchelfHook,
}: let
  version = "16.3.4";
  assets = {
    aarch64-linux = {
      name = "omp-linux-arm64";
      hash = "sha256-6c62RokhOv4b3LwX1N3/BoRLGreUA17VEsdwSLwEFq4=";
    };
    x86_64-linux = {
      name = "omp-linux-x64";
      hash = "sha256-axXfGYpTezJN1+fJOAH/EehAGSmCfaMEdr9jAheCBug=";
    };
    aarch64-darwin = {
      name = "omp-darwin-arm64";
      hash = "sha256-soPwaXaytyPIwh5L7nCcTFMpIlaoSDcfch7m+gVcr9Y=";
    };
    x86_64-darwin = {
      name = "omp-darwin-x64";
      hash = "sha256-W2ZQkfu4QyxvkY1LShb4GoTuetjw/gfFcJKyP0UIEZM=";
    };
  };
  system = stdenvNoCC.hostPlatform.system;
  asset =
    assets.${system}
    or (throw "omp: no upstream release asset for ${system}");
in
  stdenvNoCC.mkDerivation {
    pname = "omp";
    inherit version;

    src = fetchurl {
      url = "https://github.com/can1357/oh-my-pi/releases/download/v${version}/${asset.name}";
      inherit (asset) hash;
    };

    dontUnpack = true;
    dontStrip = true;

    # The binary links only glibc; patching the interpreter makes the
    # package run on hosts without nix-ld.
    nativeBuildInputs = lib.optional stdenvNoCC.hostPlatform.isLinux autoPatchelfHook;

    installPhase = ''
      # shell
      runHook preInstall
      install -Dm755 $src $out/bin/omp
      runHook postInstall
    '';

    meta = {
      description = "Oh My Pi coding agent (upstream release binary)";
      homepage = "https://github.com/can1357/oh-my-pi";
      license = lib.licenses.mit;
      mainProgram = "omp";
      platforms = builtins.attrNames assets;
      sourceProvenance = [lib.sourceTypes.binaryNativeCode];
    };
  }
