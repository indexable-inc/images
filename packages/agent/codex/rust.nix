# codex-rs built on the repo's per-unit Rust DAG (`ix.cargoUnit.buildWorkspace`),
# the same machinery that builds index's own crates. This replaces the old
# `rustPlatform.buildRustPackage` recipe so that:
#   - cross-compilation to Darwin falls out of `target = "aarch64-apple-darwin"`
#     (the RFC 0009 apple SDK toolchain), instead of needing a real Mac; and
#   - a codex-src bump rebuilds only the crates whose sources changed, not the
#     whole vendored world (the fork bumps constantly).
#
# The C/C++ heavy dependencies (`webrtc-sys`, the `v8` crate) get their prebuilt
# archives from ./prebuilt.nix through the env vars their build scripts read;
# nothing is downloaded at build time. `target = null` builds for the host.
{
  lib,
  pkgs,
  ix,
  codexSrc,
  binName ? "codex",
  # Rust target triple for a cross build (e.g. "aarch64-apple-darwin"), or null
  # to build for the host. Only the Apple-Darwin triples are wired here (the one
  # lane that needs cross); other triples would need their own toolchain branch.
  target ? null,
}: let
  inherit (pkgs) stdenv;
  inherit (ix) cargoUnit;

  isCross = target != null;
  targetIsDarwin = isCross && lib.hasSuffix "-apple-darwin" target;

  # The system whose prebuilt archives this build needs: the cross target's
  # system, or the host system for a native build.
  targetSystem =
    if !isCross
    then stdenv.hostPlatform.system
    else if targetIsDarwin
    then
      (
        if lib.hasPrefix "aarch64-" target
        then "aarch64-darwin"
        else "x86_64-darwin"
      )
    else throw "codex rust.nix: unsupported cross target ${target}";

  prebuilt = import ./prebuilt.nix {inherit (pkgs) fetchurl runCommand unzip;} targetSystem;

  # The Apple cross toolchain (zig cc + macOS SDK), or null for a native build.
  # Same wiring as lib/rust/workspace.nix `mkUnits`.
  appleToolchain =
    if targetIsDarwin
    then
      ix.appleSdkToolchain {
        appleSdk = ix.macosSdk {inherit pkgs;};
        inherit lib target;
        inherit (pkgs) writeBashApplication;
      }
    else null;

  # Git dependencies pinned in codex-rs/Cargo.lock, keyed by the exact Cargo.lock
  # source string (cargoUnit's vendorer keys by source, not name-version like
  # rustPlatform.importCargoLock). The five rust-sdks crates share one source
  # (one locked rev), so one hash covers them. Refresh after a codex-src bump by
  # rebuilding and copying the corrected hashes from the fetchgit mismatch errors.
  outputHashes = {
    "git+https://github.com/dzbarsky/rules_rust?rev=b56cbaa8465e74127f1ea216f813cd377295ad81#b56cbaa8465e74127f1ea216f813cd377295ad81" = "sha256-uJpVLcQh8wWZA3GPv9D8Nt43EOirajfDJ7eq/FB+tek=";
    "git+https://github.com/helix-editor/nucleo.git?rev=4253de9faabb4e5c6d81d946a5e35a90f87347ee#4253de9faabb4e5c6d81d946a5e35a90f87347ee" = "sha256-Hm4SxtTSBrcWpXrtSqeO0TACbUxq3gizg1zD/6Yw/sI=";
    "git+https://github.com/juberti-oai/rust-sdks.git?rev=e2d1d1d230c6fc9df171ccb181423f957bb3c1f0#e2d1d1d230c6fc9df171ccb181423f957bb3c1f0" = "sha256-0HPuwaGcqpuG+Pp6z79bCuDu/DyE858VZSYr3DKZD9o=";
    "git+https://github.com/nornagon/crossterm?rev=87db8bfa6dc99427fd3b071681b07fc31c6ce995#87db8bfa6dc99427fd3b071681b07fc31c6ce995" = "sha256-6qCtfSMuXACKFb9ATID39XyFDIEMFDmbx6SSmNe+728=";
    "git+https://github.com/nornagon/ratatui?rev=9b2ad1298408c45918ee9f8241a6f95498cdbed2#9b2ad1298408c45918ee9f8241a6f95498cdbed2" = "sha256-HBvT5c8GsiCxMffNjJGLmHnvG77A6cqEL+1ARurBXho=";
    "git+https://github.com/openai-oss-forks/tokio-tungstenite?rev=0e5b2d73aa18dd9f0a50ee9ff199d5aef7594186#0e5b2d73aa18dd9f0a50ee9ff199d5aef7594186" = "sha256-V1xmnrfRWOcZZogelZEA4vvyMj2awCfHVA5/glQ6KAI=";
    "git+https://github.com/openai-oss-forks/tungstenite-rs?rev=4fffad30fe373adbdcffab9545e9e9bf4f2fc19f#4fffad30fe373adbdcffab9545e9e9bf4f2fc19f" = "sha256-VVHhk7l9J/sEmG3q/UuV/sQ3f+fGsmq5vumSy8vbMvw=";
  };

  # `codex-rs` is a subtree of the (patched) codex source. Pass it as both the
  # build input and the workspace root, the shape cargoUnit expects for a
  # fetched/patched source (workspaceRoot = src).
  workspaceRoot = codexSrc + "/codex-rs";

  workspace = cargoUnit.buildWorkspace ({
      pname = "codex-rs${lib.optionalString isCross "-${target}"}";
      src = workspaceRoot;
      inherit workspaceRoot outputHashes;
      cargoLock.lockFile = workspaceRoot + "/Cargo.lock";
      # Match upstream's release build: the codex binary only, not the whole
      # workspace of test/support crates.
      cargoArgs = ["--package" "codex-cli"];
      cargoTargets = [["--package" "codex-cli"]];
      cargoTargetNames = ["build"];
      # codex is an external vendored build, not our own linted workspace, so
      # skip clippy/audit/machete (also what the cross graph does).
      policy = cargoUnit.policyPresets.pureBuild;
      # Input-address the whole codex graph (cargo-unit defaults to
      # `contentAddressed = true`). Codex is built once and substituted from
      # cache.ix.dev by every other machine; a floating-CA output has no
      # eval-time path, so substituting it needs the cache's `/realisations`
      # build trace, which cache.ix.dev (atticd behind ncps) 404s -- it serves
      # narinfos only. Input-addressed drvs carry concrete out paths, so plain
      # narinfo substitution works. Same rationale the cross graph documents in
      # lib/rust/workspace.nix; it holds for the native codex graph too, and it
      # also keeps the input-addressed wrapper derivation from becoming a
      # deferred CA derivation that cannot resolve after this graph's IFD.
      contentAddressed = false;
      nativeBuildInputs =
        [
          pkgs.clang
          pkgs.cmake
          pkgs.pkg-config
          pkgs.gitMinimal
          pkgs.lld
        ]
        ++ lib.optionals (appleToolchain != null) appleToolchain.runtimeInputs;
      env =
        {
          # bindgen (webrtc-sys, others) dlopens libclang and needs the header
          # search paths the Linux sandbox does not provide by default.
          LIBCLANG_PATH = "${lib.getLib pkgs.llvmPackages.libclang}/lib";
          # webrtc-sys/build reads this and links `static=webrtc` from the
          # prebuilt bundle (include/, lib/libwebrtc.a, *.ninja).
          LK_CUSTOM_WEBRTC = "${prebuilt.libwebrtc}";
          # The v8 crate's build script consumes this prebuilt static archive
          # instead of downloading it.
          RUSTY_V8_ARCHIVE = "${prebuilt.librustyV8}";
          # openssl-sys finds the system openssl through pkg-config.
          PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
          # Silence the warning-as-error false positives upstream documents
          # (GCC stringop-overflow in BoringSSL; Clang character-conversion).
          NIX_CFLAGS_COMPILE = toString (
            lib.optional stdenv.cc.isGNU "-Wno-error=stringop-overflow"
            ++ lib.optional stdenv.cc.isClang "-Wno-error=character-conversion"
          );
        }
        // lib.optionalAttrs (appleToolchain != null) appleToolchain.env;
      # Build scripts emit `-l` flags that reach the final link, but their
      # `rustc-link-search` paths do not cross cargoUnit's per-unit boundary, so
      # the native libs the codex binary links (openssl, libcap on Linux) need
      # their lib dirs added to the final link search directly.
      extraLinkRustcArgsForPlatform = _platform:
        ["-L" "native=${pkgs.openssl.out}/lib"]
        ++ lib.optionals stdenv.hostPlatform.isLinux ["-L" "native=${pkgs.libcap.lib}/lib"];
      # The v8 crate links rusty_v8 as a `+bundle` static lib, so rustc must
      # find librusty_v8.a at the *crate compile* to embed it into the v8 rlib
      # (this is where the build fails without it, not at the final link). The
      # build script's own copy lands under build_dir() and never crosses the
      # per-unit boundary; hand the compile the decompressed archive directly.
      # Scoped to `v8` so it does not perturb the rest of the closure.
      packageRustcArgs.v8 = ["-L" "native=${prebuilt.librustyV8Lib}"];
    }
    // lib.optionalAttrs isCross {
      inherit target;
      rustToolchain = ix.rustToolchainFor pkgs {
        channel = "stable";
        version = "latest";
        targets = [target];
      };
      extraRustcArgsForPlatform =
        if appleToolchain != null
        then appleToolchain.rustcArgsForPlatform
        else (_platform: []);
    });
in {
  inherit workspace;
  binary = workspace.binaries.${binName};
}
