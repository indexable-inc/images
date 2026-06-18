{
  lib,
  stdenv,
  fetchurl,
  autoPatchelfHook,
  nix,
  # Bound to a real builder only on the flake-package path (lib/packages.nix),
  # which is where `nix run .#humanlayer.updateScript` resolves; the overlay path
  # passes nothing, so `pkgs.humanlayer` carries no updateScript. Same pattern as
  # packages/yc and packages/agent/claude-code.
  writeNushellApplication ? null,
}:
let
  # Version and per-platform hashes live in manifest.json as the single owner;
  # this file only reads them back. Refresh with
  # `nix run .#humanlayer.updateScript` (see the updateScript below). Mirrors the
  # packages/yc layout.
  manifest = lib.importJSON ./manifest.json;
  inherit (manifest) version;

  # The `@humanlayer/cli` npm launcher is a thin JS shim that execs a
  # platform-specific, bun-compiled standalone binary shipped in a sibling
  # package (`@humanlayer/cli-<slug>`). We fetch that platform binary directly:
  # it is the real `humanlayer` executable, so the JS launcher is not needed.
  baseUrl = "https://registry.npmjs.org/@humanlayer";

  inherit (stdenv.hostPlatform) system;
  target =
    manifest.platforms.${system}
      or (throw "humanlayer: no prebuilt binary for ${system}; supported: ${lib.concatStringsSep ", " (builtins.attrNames manifest.platforms)}");

  src = fetchurl {
    url = "${baseUrl}/cli-${target.slug}/-/cli-${target.slug}-${version}.tgz";
    inherit (target) hash;
  };

  # Tracks the upstream npm `latest` dist-tag and refreshes manifest.json with
  # the per-platform SRI hashes. HumanLayer publishes no signed manifest, so
  # there is NO provenance check: the updater pins whatever bytes the npm
  # registry serves. The `nix build .#humanlayer` step in the update workflow
  # only proves the pinned bytes fetch and patch on x86_64-linux, not that they
  # are authentic, so the real gate is human review of the hash changes in the
  # auto-bump PR. Same posture as packages/yc.
  updateScript =
    if writeNushellApplication == null then
      null
    else
      writeNushellApplication {
        name = "humanlayer-update";
        runtimeInputs = [ nix ];
        meta.description = "Refresh packages/humanlayer/manifest.json to the latest HumanLayer CLI release";
        text = ''
          const base = "https://registry.npmjs.org/@humanlayer"
          const slugs = {
            "x86_64-linux": "linux-x64",
            "aarch64-linux": "linux-arm64",
            "aarch64-darwin": "darwin-arm64",
            "x86_64-darwin": "darwin-x64"
          }

          # Run from the repo root: `nix run .#humanlayer.updateScript -- [version]`.
          # Without a version argument it tracks the upstream npm `latest` tag.
          def main [version?: string] {
            let v = ($version | default (http get $"($base)/cli/latest" | get version))
            let platforms = (
              $slugs
              | transpose system slug
              | reduce --fold {} {|row acc|
                  let url = $"($base)/cli-($row.slug)/-/cli-($row.slug)-($v).tgz"
                  let sri = (^nix store prefetch-file --json $url | from json | get hash)
                  $acc | insert $row.system { slug: $row.slug, hash: $sri }
                }
            )
            let out = "packages/humanlayer/manifest.json"
            { version: $v, platforms: $platforms } | to json --indent 2 | save --force $out
            print $"updated ($out) to ($v)"
          }
        '';
      };
in
stdenv.mkDerivation {
  pname = "humanlayer";
  inherit version src;

  # The npm tarball unpacks to `package/` (bin/humanlayer + package.json).
  sourceRoot = "package";

  # The Linux binaries are dynamically linked against a generic libc; patch their
  # interpreter and standard library NEEDEDs to the Nix store. Darwin binaries
  # need no patching.
  nativeBuildInputs = lib.optional stdenv.hostPlatform.isLinux autoPatchelfHook;

  installPhase = ''
    runHook preInstall
    install -Dm755 bin/humanlayer $out/bin/humanlayer
    runHook postInstall
  '';

  passthru = lib.optionalAttrs (updateScript != null) {
    inherit updateScript;
  };

  meta = {
    description = "HumanLayer CLI: manage the riptide remote daemon, agents, and HumanLayer API";
    homepage = "https://humanlayer.com";
    # License omitted rather than `licenses.unfree` so the per-system flake
    # package set (which evaluates nixpkgs without `allowUnfree`) can still
    # `nix run .#humanlayer`. Same posture as packages/yc. Distribution terms are
    # HumanLayer's; this flake only repackages the published binaries.
    mainProgram = "humanlayer";
    platforms = builtins.attrNames manifest.platforms;
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
  };
}
