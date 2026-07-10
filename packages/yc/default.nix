{
  lib,
  stdenv,
  fetchurl,
  autoPatchelfHook,
  nix,
  # Writer used to build `passthru.updateScript`. Bound to a real builder only on
  # the flake-package path (lib/packages.nix), which is where
  # `nix run .#yc.updateScript` resolves; the overlay path leaves it null, so
  # `pkgs.yc` carries no updateScript. Same pattern as claude-code.
  updateScriptWriter ? null,
}: let
  # Slug map and pinned hashes live in manifest.json as the single owner; this
  # file only reads them back. Refresh with `nix run .#yc.updateScript` (see the
  # updateScript below). Mirrors the packages/agent/claude-code layout.
  manifest = lib.importJSON ./manifest.json;
  inherit (manifest) version;

  baseUrl = "https://bookface-public.s3.us-west-2.amazonaws.com/cli";

  inherit (stdenv.hostPlatform) system;
  target =
    manifest.platforms.${system}
      or (throw "yc: no prebuilt binary for ${system}; supported: ${lib.concatStringsSep ", " (builtins.attrNames manifest.platforms)}");

  src = fetchurl {
    url = "${baseUrl}/${version}/yc-${target.slug}";
    inherit (target) hash;
  };

  # Tracks the upstream `cli/latest` pointer (a 6-byte text file holding the
  # version, e.g. "0.0.8") and refreshes manifest.json with the per-platform SRI
  # hashes. Unlike claude-code, Y Combinator publishes no signed manifest, so
  # there is NO provenance check: the updater pins whatever bytes the release
  # host serves (the version tag is not even immutable, 0.0.5 was observed
  # republished under the same tag). The `nix build .#yc` step in the update
  # workflow only proves the pinned bytes fetch and patch, not that they are
  # authentic, so the real gate is human review of the four hash changes in the
  # auto-bump PR. The S3 bucket denies ListBucket, hence the `latest` pointer
  # rather than enumerating versions.
  updateScript =
    if updateScriptWriter == null
    then null
    else
      updateScriptWriter {
        name = "yc-update";
        runtimeInputs = [nix];
        meta.description = "Refresh packages/yc/manifest.json to the latest YC CLI release";
        text = ''
          # nu
          const base = "${baseUrl}"
          const slugs = {
            "aarch64-darwin": "darwin-arm64",
            "x86_64-darwin": "darwin-x64",
            "x86_64-linux": "linux-x64",
            "aarch64-linux": "linux-arm64"
          }

          # Run from the repo root: `nix run .#yc.updateScript -- [version]`.
          # Without a version argument it tracks the upstream `cli/latest` pointer.
          def main [version?: string] {
            let v = ($version | default (http get $"($base)/latest" | str trim))
            let platforms = (
              $slugs
              | transpose system slug
              | reduce --fold {} {|row acc|
                  let url = $"($base)/($v)/yc-($row.slug)"
                  let sri = (^nix store prefetch-file --json $url | from json | get hash)
                  $acc | insert $row.system { slug: $row.slug, hash: $sri }
                }
            )
            let out = "packages/yc/manifest.json"
            { version: $v, platforms: $platforms } | to json --indent 2 | save --force $out
            print $"updated ($out) to ($v)"
          }
        '';
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
      # shell
      runHook preInstall
      install -Dm755 $src $out/bin/yc
      runHook postInstall
    '';

    passthru = lib.optionalAttrs (updateScript != null) {
      inherit updateScript;
    };

    meta = {
      description = "YC CLI: search Bookface and chat with the YC Agent from the terminal";
      homepage = "https://bookface.ycombinator.com";
      # License omitted rather than `licenses.unfree` so the per-system flake
      # package set (which evaluates nixpkgs without `allowUnfree`) can still
      # `nix run .#yc`. Same posture as packages/agent/claude-code. Distribution terms
      # are Y Combinator's; this flake only repackages the published binaries.
      mainProgram = "yc";
      platforms = builtins.attrNames manifest.platforms;
      sourceProvenance = [lib.sourceTypes.binaryNativeCode];
    };
  }
