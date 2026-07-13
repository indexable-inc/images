# Prebuilt native artifacts that codex's build scripts would otherwise download
# from the internet at build time. The nix sandbox has no network, so they are
# pinned here as fixed-output derivations and handed to the build scripts through
# the env vars they already consult (RUSTY_V8_ARCHIVE, LK_CUSTOM_WEBRTC).
#
# Keyed by the *target* system, not the build host: a Linux->Darwin cross build
# of codex must fetch the Darwin archives. Versions track codex's Cargo.lock:
#   - `v8` crate 149.2.0            -> denoland/rusty_v8 release v149.2.0
#   - webrtc-sys-build WEBRTC_TAG   -> livekit/rust-sdks release webrtc-24f6822-2
# Refresh both alongside a codex-src bump: read the new v8 version out of
# codex-rs/Cargo.lock and the new WEBRTC_TAG out of
# webrtc-sys/build/src/lib.rs, then re-run `nix store prefetch-file` on the URLs.
{
  fetchurl,
  runCommand,
  unzip,
}: targetSystem: let
  onlyKnown = attr: name:
    attr.${targetSystem}
    or (throw "codex prebuilt ${name} has no pin for target system ${targetSystem}");

  # denoland/rusty_v8 static archive matching the `v8` crate. The v8 build script
  # consumes the gzipped `.a` directly through RUSTY_V8_ARCHIVE (the same store
  # path shape nixpkgs' codex feeds it), so no decompression step here.
  rustyV8Version = "149.2.0";
  rustcTarget = onlyKnown {
    x86_64-linux = "x86_64-unknown-linux-gnu";
    aarch64-linux = "aarch64-unknown-linux-gnu";
    aarch64-darwin = "aarch64-apple-darwin";
  } "rustcTarget";
  rustyV8Hash = onlyKnown {
    x86_64-linux = "sha256-iu2YY323533Iv7i7R1nsW95HLQv3lD9Y4OYqNQlFxVk=";
    aarch64-linux = "sha256-+XdRJ8pk3MSjZi0BpSGizvuluY+DOUOog9hHc7Kv88U=";
    aarch64-darwin = "sha256-+rsuyNO6Wm3qY9uaNalg3FypheujLzQrm6Sqocc0sv4=";
  } "librusty_v8 hash";

  # livekit/rust-sdks prebuilt static libwebrtc. The zip carries include/,
  # lib/libwebrtc.a and the webrtc.ninja / desktop_capture.ninja manifests that
  # webrtc-sys/build reads for its preprocessor defines; LK_CUSTOM_WEBRTC points
  # at the extracted root and the build then links `static=webrtc`.
  webrtcTag = "webrtc-24f6822-2";
  webrtcTriple = onlyKnown {
    x86_64-linux = "linux-x64-release";
    aarch64-linux = "linux-arm64-release";
    aarch64-darwin = "mac-arm64-release";
  } "webrtc triple";
  webrtcHash = onlyKnown {
    x86_64-linux = "sha256-89SaZMN+qJmvUt3GhfUx8Kvi+3VSiqTa4lKtqqA77Mw=";
    aarch64-linux = "sha256-QBPVPoY+RwQt1Ztnsb2EltoER6yEw9cMFwSZQG8Tqgs=";
    aarch64-darwin = "sha256-eb5cwV5uBjPEOA4z4XLX6/Gm3Og+ngmXYdYQPw1+tsE=";
  } "webrtc hash";
  rustyV8Archive = fetchurl {
    name = "librusty_v8-${rustyV8Version}-${targetSystem}.a.gz";
    url = "https://github.com/denoland/rusty_v8/releases/download/v${rustyV8Version}/librusty_v8_release_${rustcTarget}.a.gz";
    hash = rustyV8Hash;
  };
in {
  librustyV8 = rustyV8Archive;

  # Decompressed `librusty_v8.a` in its own dir, for the v8 crate compile's
  # `-L native=` search. The v8 build script does write a decompressed copy, but
  # under `build_dir()` (an ancestor of OUT_DIR) which does not cross cargoUnit's
  # per-unit boundary, so the compile cannot see it. rustc needs the file named
  # exactly `librusty_v8.a` to satisfy `-l static=rusty_v8`.
  librustyV8Lib = runCommand "librusty_v8-${rustyV8Version}-${targetSystem}-lib" {} ''
    mkdir -p "$out"
    gzip -dc ${rustyV8Archive} > "$out/librusty_v8.a"
  '';

  # fetchurl (flat file hash) + explicit unzip rather than fetchzip: the archive
  # ships prebuilt binaries whose flat hash is stable and prefetchable, and the
  # extracted tree is used as-is. Descend one level if the zip wraps its payload
  # in a single top directory, so LK_CUSTOM_WEBRTC always names the dir that
  # directly holds include/ and lib/.
  libwebrtc = let
    zip = fetchurl {
      name = "livekit-webrtc-${webrtcTriple}.zip";
      url = "https://github.com/livekit/rust-sdks/releases/download/${webrtcTag}/webrtc-${webrtcTriple}.zip";
      hash = webrtcHash;
    };
  in
    runCommand "livekit-webrtc-${webrtcTag}-${webrtcTriple}" {
      nativeBuildInputs = [unzip];
    } ''
      # shell
      unpack=$(mktemp -d)
      unzip -q ${zip} -d "$unpack"
      root="$unpack"
      if [ ! -d "$root/include" ]; then
        inner=$(find "$unpack" -maxdepth 1 -mindepth 1 -type d | head -n1)
        [ -n "$inner" ] && root="$inner"
      fi
      if [ ! -d "$root/include" ] || [ ! -d "$root/lib" ]; then
        echo "livekit webrtc archive layout unexpected under $root:" >&2
        ls -la "$root" >&2
        exit 1
      fi
      cp -R "$root" "$out"
    '';
}
