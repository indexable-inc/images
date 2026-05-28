{
  lib,
  stdenv,
  fetchurl,
  makeBinaryWrapper,
  autoPatchelfHook,
  procps,
  ripgrep,
  bubblewrap,
  socat,
  binName ? "claude",
}:

let
  # Pinned to a prerelease build on purpose. Anthropic publishes new Claude
  # Code versions to the npm `next` (prerelease) tag days before promoting
  # them to `latest` (stable), and every channel that normally surfaces an
  # upgrade only watches `latest`: the built-in `claude` auto-updater and
  # `claude doctor` both reported "up to date", and sadjow/claude-code-nix
  # (the usual Nix source) tracks stable too. At the Opus 4.8 launch 2.1.154
  # sat on `next` while everything else still showed 2.1.153, yet 2.1.154 is
  # the first build that defaults `/fast` to Opus 4.8. Fetching the platform
  # binary directly by version is the only way to pin ahead of the stable
  # promotion. When `latest` catches up, this can fall back to a channel that
  # tracks stable. Bump by re-prefetching each platform binary:
  #   nix store prefetch-file --json \
  #     https://downloads.claude.ai/claude-code-releases/<version>/<slug>/claude
  version = "2.1.154";

  # Claude Code ships prebuilt Bun single-file executables per platform. The
  # download path keys off Anthropic's own platform slugs rather than the Nix
  # system doubles, so map between them here.
  platforms = {
    aarch64-darwin = {
      slug = "darwin-arm64";
      hash = "sha256-vJiBsQfXvhdDxkyLct1meY9dCUfbxI7Q13lkxHNmH9Q=";
    };
    x86_64-darwin = {
      slug = "darwin-x64";
      hash = "sha256-FgjZMmGHkgHc933TLcFz777qcVGH01Qv0Fr899W17E0=";
    };
    x86_64-linux = {
      slug = "linux-x64";
      hash = "sha256-Z/bKt+bBJAEPYqwY+AeLwJ4NtqW56K6HTp5zAzxFF5M=";
    };
    aarch64-linux = {
      slug = "linux-arm64";
      hash = "sha256-n3Mt4nj3rcYdKf1bBV3a8brjuybXX+bgahJWAlZXd6g=";
    };
  };

  inherit (stdenv.hostPlatform) system;
  target =
    platforms.${system}
      or (throw "claude-code: no prebuilt binary for ${system}; supported: ${lib.concatStringsSep ", " (builtins.attrNames platforms)}");

  # Primary host is the Anthropic-branded CDN so the source is verifiable; the
  # GCS bucket is the direct origin and stays as a mirror if the CDN is down.
  # The hash pin guarantees both resolve to identical bytes, so this is a
  # mirror list, not a behavioral fallback.
  nativeBinary = fetchurl {
    urls = [
      "https://downloads.claude.ai/claude-code-releases/${version}/${target.slug}/claude"
      "https://storage.googleapis.com/claude-code-dist-86c565f3-f756-42ad-8dfa-d59b1c096819/claude-code-releases/${version}/${target.slug}/claude"
    ];
    inherit (target) hash;
  };
in
stdenv.mkDerivation {
  pname = "claude-code";
  inherit version;

  dontUnpack = true;
  # Stripping rewrites the binary and corrupts the trailer Bun appends to its
  # single-file executables, so the stripped CLI aborts on launch.
  dontStrip = true;
  strictDeps = true;

  nativeBuildInputs = [
    makeBinaryWrapper
  ]
  ++ lib.optional stdenv.hostPlatform.isElf autoPatchelfHook;

  installPhase = ''
    runHook preInstall
    mkdir -p $out/bin

    install -m755 ${nativeBinary} $out/bin/.${binName}-unwrapped

    # The store output is read-only, so the bundled self-updater can never
    # write; disable it and the install checks, and pin the bundled ripgrep to
    # the Nix one so PATH stays reproducible. The wrapper owns the version pin.
    makeBinaryWrapper $out/bin/.${binName}-unwrapped $out/bin/${binName} \
      --inherit-argv0 \
      --set DISABLE_AUTOUPDATER 1 \
      --set DISABLE_INSTALLATION_CHECKS 1 \
      --set USE_BUILTIN_RIPGREP 0 \
      --prefix PATH : ${
        lib.makeBinPath (
          [
            procps
            ripgrep
          ]
          ++ lib.optionals stdenv.hostPlatform.isLinux [
            bubblewrap
            socat
          ]
        )
      }

    runHook postInstall
  '';

  meta = {
    description = "Claude Code, Anthropic's agentic coding tool in the terminal";
    homepage = "https://www.anthropic.com/claude-code";
    # License omitted rather than `licenses.unfree` to match the proprietary
    # `packages/ix` vendor binary: the per-system flake package set evaluates
    # nixpkgs without `allowUnfree`, so tagging it unfree would block
    # `nix run .#claude-code`. Distribution terms are Anthropic's commercial
    # Claude Code license.
    mainProgram = binName;
    platforms = builtins.attrNames platforms;
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
  };
}
