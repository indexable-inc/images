# Cross-compile a Rust workspace (and its C/C++ deps) from Linux to Darwin.
#
# zig is the cross C/C++ compiler (`zig cc` / `zig c++`) with the macOS SDK as
# its sysroot; `clang -fuse-ld=lld` is the Rust linker. The returned `env` is
# merged into `cargoUnit.buildWorkspace`'s build environment, where the
# nix-cargo-unit renderer picks up `CARGO_TARGET_<T>_LINKER` per unit and
# threads `--target` into rustc itself (see packages/nix-cargo-unit/src/render.rs).
#
# Ported from the sibling `ix` repo (nix/lib/apple-sdk-toolchain.nix). The wrapper
# scripts use `writeTextFile` rather than `writeShellApplication` because the
# latter is lint-banned here; a `bash -n` checkPhase keeps the syntax checked.
{
  appleSdk,
  lib,
  pkgs,
  target,
}:
let
  supportedTargets = [
    "aarch64-apple-darwin"
    "x86_64-apple-darwin"
  ];
  targetEnvName = lib.replaceStrings [ "-" ] [ "_" ] target;
  cargoTargetEnvName = lib.toUpper targetEnvName;
  zigTarget = if target == "aarch64-apple-darwin" then "aarch64-macos" else "x86_64-macos";
  cmakeArch = if target == "aarch64-apple-darwin" then "arm64" else "x86_64";
  targetFlags = "-Wno-error=nullability-completeness -Wno-nullability-completeness -isysroot ${appleSdk} -I${appleSdk}/usr/include -iframework ${appleSdk}/System/Library/Frameworks -F${appleSdk}/System/Library/Frameworks";

  # `zig cc` records Apple triples in clang spelling (`arm64-apple-macosx`),
  # but zig's own `--target` flag wants `aarch64-macos`. Rewrite any inbound
  # `--target=` that cargo/cc-rs passes through so zig accepts it.
  normalizeTargetFunction = ''
    normalize_target() {
      case "$1" in
        --target=arm64-apple-macosx)
          printf '%s' '--target=aarch64-macos'
          ;;
        --target=arm64-apple-macosx*)
          printf '%s' "--target=aarch64-macos.''${1#--target=arm64-apple-macosx}"
          ;;
        --target=x86_64-apple-macosx)
          printf '%s' '--target=x86_64-macos'
          ;;
        --target=x86_64-apple-macosx*)
          printf '%s' "--target=x86_64-macos.''${1#--target=x86_64-apple-macosx}"
          ;;
        *)
          printf '%s' "$1"
          ;;
      esac
    }
  '';

  # Sanitizer instrumentation is not wired up for the cross Darwin toolchain;
  # drop the flags cargo/cc-rs may inject so a sanitizer build of a host unit
  # does not poison the cross C compile.
  sanitizerFilterFunction = ''
    should_drop_apple_sanitizer_arg() {
      case "$1" in
        -fsanitize=*|-fno-sanitize=*|-fsanitize-trap=*|-fno-sanitize-trap=*|-shared-libsan|-static-libsan|-rtlib=* )
          return 0
          ;;
        -Wl,-fsanitize=*|-Wl,-fno-sanitize=*|-Wl,-fsanitize-trap=*|-Wl,-fno-sanitize-trap=* )
          return 0
          ;;
        -Wp,-U_FORTIFY_SOURCE )
          return 0
          ;;
        *)
          return 1
          ;;
      esac
    }
  '';

  # zig cc fails before producing any diagnostic the caller can see when it
  # can't write its global cache directory. Under a sandboxed service user's
  # HOME=/var/empty, the default $HOME/.cache/zig is unwritable, so zig
  # exits 1 with `unable to open global cache directory ... ReadOnlyFileSystem`.
  # cc-rs in the calling Rust build script swallows that stderr and only
  # prints its own "error occurred in cc-rs" line, so the action's stderr
  # arrives empty and the failure looks like a silent compiler bug. Pin
  # ZIG_GLOBAL_CACHE_DIR to an action-local writable path here so the wrapper
  # is self-contained regardless of caller HOME and avoids sharing mutable
  # cache state between concurrent builds. See ix ENG-1278.
  #
  # No public upstream bug exists for this exact failure mode: it is the
  # interaction of three independently-correct behaviors. Zig's documented
  # contract is that the global cache directory must be writable and is
  # selectable via ZIG_GLOBAL_CACHE_DIR (https://ziglang.org/documentation/0.16.0/#Compile-Cache).
  # NixOS gives system users HOME=/var/empty by default. systemd ProtectHome=true
  # is intentional sandbox hardening
  # (https://www.freedesktop.org/software/systemd/man/systemd.exec.html#ProtectHome=).
  # cc-rs emits only the failed command on a non-zero exit (see Error::ToolExecError
  # in https://docs.rs/cc/latest/cc/struct.Error.html).
  zigCachePreamble = ''
    export ZIG_GLOBAL_CACHE_DIR="''${ZIG_GLOBAL_CACHE_DIR:-''${TMPDIR:-$PWD}/zig-cache}"
    if ! mkdir -p "$ZIG_GLOBAL_CACHE_DIR" 2>/dev/null; then
      printf 'apple-sdk-cc: failed to create zig cache at %s (HOME=%s TMPDIR=%s)\n' \
        "$ZIG_GLOBAL_CACHE_DIR" "''${HOME:-<unset>}" "''${TMPDIR:-<unset>}" >&2
      exit 65
    fi
  '';

  # Shared argument-rewriting loop: normalize `--target=`, drop `-arch <a>` and
  # `-m64` (Apple/gcc spellings zig rejects), strip sanitizer args, and remember
  # whether the caller already supplied a target.
  argLoop = ''
    apple_args=()
    has_target=0
    drop_next=0
    ${normalizeTargetFunction}
    ${sanitizerFilterFunction}
    for arg in "$@"; do
      if [ "$drop_next" -eq 1 ]; then
        drop_next=0
        continue
      fi
      case "$arg" in
        -arch)
          drop_next=1
          ;;
        -m64)
          ;;
        --target=*)
          has_target=1
          apple_args+=("$(normalize_target "$arg")")
          ;;
        *)
          if should_drop_apple_sanitizer_arg "$arg"; then
            continue
          fi
          apple_args+=("$arg")
          ;;
      esac
    done
  '';

  frameworkFlags = "-isysroot ${appleSdk} -I${appleSdk}/usr/include -iframework ${appleSdk}/System/Library/Frameworks -F${appleSdk}/System/Library/Frameworks";

  zig = lib.getExe pkgs.zig;
  clang = lib.getExe' pkgs.llvmPackages.clang-unwrapped "clang";
  ar = lib.getExe' pkgs.llvmPackages.bintools "ar";
  ranlib = lib.getExe' pkgs.llvmPackages.bintools "ranlib";

  # writeTextFile + `bash -n` checkPhase: a checked replacement for the
  # lint-banned writeShellApplication for these internal build wrappers.
  mkScript =
    name: text:
    pkgs.writeTextFile {
      inherit name;
      executable = true;
      destination = "/bin/${name}";
      text = ''
        #!${pkgs.runtimeShell}
        set -euo pipefail
        ${text}
      '';
      checkPhase = ''${lib.getExe' pkgs.bash "bash"} -n "$out/bin/${name}"'';
    };
  exeOf = pkg: name: "${pkg}/bin/${name}";

  ccName = "apple-sdk-cc-${target}";
  cxxName = "apple-sdk-cxx-${target}";
  linkerName = "apple-sdk-linker-${target}";
  arName = "apple-sdk-ar-${target}";
  ranlibName = "apple-sdk-ranlib-${target}";

  appleCcPackage = mkScript ccName ''
    ${zigCachePreamble}
    ${argLoop}
    if [ "$has_target" -eq 0 ]; then
      apple_args=("--target=${zigTarget}" "''${apple_args[@]}")
    fi
    exec ${zig} cc -mmacosx-version-min=11.0 ${frameworkFlags} "''${apple_args[@]}" -fno-sanitize=undefined
  '';
  appleCxxPackage = mkScript cxxName ''
    ${zigCachePreamble}
    ${argLoop}
    if [ "$has_target" -eq 0 ]; then
      apple_args=("--target=${zigTarget}" "''${apple_args[@]}")
    fi
    exec ${zig} c++ -mmacosx-version-min=11.0 ${frameworkFlags} "''${apple_args[@]}" -fno-sanitize=undefined
  '';
  appleLinkerPackage = mkScript linkerName ''
    ${argLoop}
    if [ "$has_target" -eq 0 ]; then
      apple_args=("--target=${target}" "''${apple_args[@]}")
    fi
    exec ${clang} -B${pkgs.lld}/bin -fuse-ld=lld -mmacosx-version-min=11.0 -isysroot ${appleSdk} -Wno-unused-command-line-argument "''${apple_args[@]}" -fno-sanitize=undefined
  '';
  appleArPackage = mkScript arName ''exec ${ar} "$@"'';
  appleRanlibPackage = mkScript ranlibName ''exec ${ranlib} "$@"'';
  appleXcrun = mkScript "xcrun" ''
    if [ "$#" -eq 3 ] && [ "$1" = "--sdk" ] && [ "$2" = "macosx" ] && [ "$3" = "--show-sdk-path" ]; then
      printf '%s\n' ${appleSdk}
      exit 0
    fi
    echo "unsupported xcrun invocation: $*" >&2
    exit 1
  '';

  appleCc = exeOf appleCcPackage ccName;
  appleCxx = exeOf appleCxxPackage cxxName;
  appleLinker = exeOf appleLinkerPackage linkerName;
  appleAr = exeOf appleArPackage arName;
  appleRanlib = exeOf appleRanlibPackage ranlibName;

  appleCmakeToolchain = pkgs.writeText "apple-sdk-toolchain-${target}.cmake" ''
    set(CMAKE_SYSTEM_NAME Darwin)
    set(CMAKE_SYSTEM_PROCESSOR ${cmakeArch})
    set(CMAKE_OSX_ARCHITECTURES ${cmakeArch})
    set(CMAKE_OSX_DEPLOYMENT_TARGET 11.0)
    set(CMAKE_OSX_SYSROOT ${appleSdk})
    set(CMAKE_C_COMPILER ${appleCc})
    set(CMAKE_CXX_COMPILER ${appleCxx})
    set(CMAKE_AR ${appleAr})
    set(CMAKE_RANLIB ${appleRanlib})
    set(CMAKE_FIND_ROOT_PATH ${appleSdk})
    set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
    set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
    set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
    set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)
  '';
in
assert lib.assertMsg (builtins.elem target supportedTargets)
  "ix.appleSdkToolchain: unsupported Apple SDK target: ${target} (supported: ${lib.concatStringsSep ", " supportedTargets})";
{
  # `--target`-aware rustc args for this platform. nix-cargo-unit's renderer
  # calls the workspace `extraRustcArgsForPlatform` hook with each unit's
  # platform string; return the framework search path only for this Darwin
  # target so host (platform=null) and other-target units are unaffected.
  rustcArgsForPlatform =
    platform:
    lib.optionals (platform == target) [
      "-L"
      "framework=${appleSdk}/System/Library/Frameworks"
    ];

  runtimeInputs = [
    appleArPackage
    appleCcPackage
    appleCxxPackage
    appleLinkerPackage
    appleRanlibPackage
    appleXcrun
    pkgs.llvmPackages.bintools-unwrapped
  ];

  env = {
    AR = appleAr;
    CC = appleCc;
    CFLAGS = targetFlags;
    CMAKE_TOOLCHAIN_FILE = appleCmakeToolchain;
    CXX = appleCxx;
    CXXFLAGS = targetFlags;
    MACOSX_DEPLOYMENT_TARGET = "11.0";
    RANLIB = appleRanlib;
    SDKROOT = appleSdk;
    "AR_${targetEnvName}" = appleAr;
    "CC_${targetEnvName}" = appleCc;
    "CMAKE_TOOLCHAIN_FILE_${targetEnvName}" = appleCmakeToolchain;
    "CXX_${targetEnvName}" = appleCxx;
    "CARGO_TARGET_${cargoTargetEnvName}_AR" = appleAr;
    "CARGO_TARGET_${cargoTargetEnvName}_LINKER" = appleLinker;
    "CFLAGS_${targetEnvName}" = targetFlags;
    "CXXFLAGS_${targetEnvName}" = targetFlags;
    "RANLIB_${targetEnvName}" = appleRanlib;
  };
}
