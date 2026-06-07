{
  lib,
  stdenv,
  fetchurl,
  autoPatchelfHook,
}:
let
  # Slug map and pinned hashes live in manifest.json as the single owner; this
  # file only reads them back. Bump by editing the version and refreshing the
  # four hashes (the upstream binaries are republished per release at the S3
  # bucket below). Mirrors the packages/claude-code layout.
  manifest = lib.importJSON ./manifest.json;
  inherit (manifest) version;

  inherit (stdenv.hostPlatform) system;
  target =
    manifest.platforms.${system}
      or (throw "yc: no prebuilt binary for ${system}; supported: ${lib.concatStringsSep ", " (builtins.attrNames manifest.platforms)}");

  src = fetchurl {
    url = "https://bookface-public.s3.us-west-2.amazonaws.com/cli/${version}/yc-${target.slug}";
    inherit (target) hash;
  };
in
stdenv.mkDerivation {
  pname = "yc";
  inherit version src;
  dontUnpack = true;

  # The Linux binaries are dynamically linked against a generic libc; patch their
  # interpreter to the Nix store. Darwin binaries need no patching.
  nativeBuildInputs = lib.optional stdenv.hostPlatform.isLinux autoPatchelfHook;

  installPhase = ''
    runHook preInstall
    install -Dm755 $src $out/bin/yc
    runHook postInstall
  '';

  meta = {
    description = "YC CLI: search Bookface and chat with the YC Agent from the terminal";
    homepage = "https://bookface.ycombinator.com";
    # License omitted rather than `licenses.unfree` so the per-system flake
    # package set (which evaluates nixpkgs without `allowUnfree`) can still
    # `nix run .#yc`. Same posture as packages/claude-code. Distribution terms
    # are Y Combinator's; this flake only repackages the published binaries.
    mainProgram = "yc";
    platforms = builtins.attrNames manifest.platforms;
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
  };
}
