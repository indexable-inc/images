# Linux -> Darwin nixpkgs cross scope for C/C++ closures that build through
# upstream nixpkgs packaging instead of the cargo-unit lane (RFC 0009). The
# consumer today is `nix-ix`: packages/nix/nix swaps its component scope to
# this one when the registry cross lane instantiates it, so the full modular
# nix closure builds on the linux fleet and a Mac only substitutes (#3585).
#
# Upstream declares Linux-hosted Darwin cross unsupported (the nixpkgs manual
# platform table; NixOS/nixpkgs#405893 keeps deferring the port). What breaks
# is one family: nixpkgs strips license-encumbered pieces out of its fetched
# SDK (metadata/disallowed-packages.json) and rebuilds them plus the apple
# toolchain from Apple OSS source, and those source builds (ld64, cctools,
# libresolv, copyfile, Csu, libsbuf, cups-headers, xcbuild, ...) compile
# Apple-only source with the linux gcc of the `pkgsBuildHost` stage. This
# repo already ships a complete pinned SDK for the rust cross lane
# (`ix.macosSdk`, lib/darwin/pins.json), which contains everything that
# family recreates -- so the scope is stock nixpkgs cross plus one overlay
# replacing the family at its three roots:
#
#   apple-sdk -> the pinned real SDK presented through upstream's own
#                contract (layout, sdk-hook setup hook, `sdkroot` passthru),
#                without the propagate-inputs/propagate-xcrun phases whose
#                only purpose is re-adding what upstream stripped. Same
#                licensing posture as the rust lane, which links this SDK's
#                raw tbd stubs already.
#   ld64      -> lld's `ld64.lld` Mach-O driver. `darwin.binutils-unwrapped`
#                consumes plain `${ld64}/bin/ld` and keeps its cctools-shaped
#                wrapper handling, so the swap is invisible downstream, and
#                ld64.lld ad-hoc-signs arm64 output on its own.
#   cctools   -> the llvm equivalents of the tools the darwin binutils
#                assembly actually takes from cctools (ranlib, lipo,
#                install_name_tool, libtool; nm/otool/strip already come
#                from llvm there). Tools with no llvm equivalent
#                (codesign_allocate, gprof) are omitted: the assembly links
#                tools behind existence checks and nothing in this lane
#                invokes them.
#
# The overlay applies to every stage of the scope, including darwin-hosted
# `targetPackages` copies nothing in this lane ever executes; that keeps the
# replacement total instead of special-casing stages.
#
# Two-stage signature like lib/darwin/macos-sdk.nix: lib/default.nix applies
# the shared `pins` reader once at import; the public `ix.darwinCrossPkgs`
# surface stays `{ pkgs, target }: package set`.
{
  macosSdk,
  pins,
}: pkgs: target: let
  sdkPin = pins.loadPin ./pins.json "macos-sdk";
  sdkRoot = macosSdk {inherit pkgs;};
  shims = _final: prev: let
    inherit (prev) lib;
    llvmPkgs = prev.llvmPackages;
    inherit (prev.stdenv) hostPlatform targetPlatform;
    targetPrefix = lib.optionalString (targetPlatform != hostPlatform) "${targetPlatform.config}-";
  in {
    apple-sdk = lib.makeOverridable (
      {
        # The darwin scope's mkBootstrapStdenv rewrites any apple-sdk-named
        # extraBuildInput with `.override { enableBootstrap = true; }` to
        # strip SDK propagation; this shim never propagates, so both
        # variants are the same derivation and the flag only needs to be
        # accepted.
        enableBootstrap ? false,
      }:
        assert lib.assertMsg (lib.isBool enableBootstrap) "apple-sdk shim: enableBootstrap must be a bool";
          prev.stdenvNoCC.mkDerivation (finalAttrs: let
            sdkName = "MacOSX${lib.versions.majorMinor finalAttrs.version}.sdk";
          in {
            pname = "apple-sdk";
            inherit (sdkPin) version;

            # The SDK content is the shared `ix.macosSdk` unpack; each stage's
            # package is a symlink farm plus hooks, so the multi-GiB tree exists
            # once in the store no matter how many stages force their copy.
            dontUnpack = true;
            dontConfigure = true;
            dontBuild = true;
            strictDeps = true;

            # Upstream's own hooks so consumers see the exact same contract
            # (DEVELOPER_DIR / SDKROOT / NIX_APPLE_SDK_VERSION per role).
            setupHooks = [
              (prev.path + "/pkgs/by-name/ap/apple-sdk/setup-hooks/role.bash")
              (prev.substitute {
                src = prev.path + "/pkgs/by-name/ap/apple-sdk/setup-hooks/sdk-hook.sh";
                substitutions = [
                  "--subst-var-by"
                  "sdkVersion"
                  (lib.escapeShellArgs (lib.splitVersion finalAttrs.version))
                ];
              })
            ];

            installPhase = ''
              # shell
              runHook preInstall

              platformPath="$out/Platforms/MacOSX.platform"
              sdkpath="$platformPath/Developer/SDKs"
              mkdir -p "$sdkpath"
              sdkDir="$sdkpath/${sdkName}"
              # A selective symlink view of the SDK rather than one root link:
              # the real SDK ships its own (older) libc++ headers under
              # usr/include/c++, while this scope's C++ headers come from
              # `darwin.libcxx` (the LLVM ones); mixing the trees breaks the
              # moment libc++'s stdlib.h include_next lands in the SDK copy
              # (`_LIBCPP_NODISCARD` no longer exists in LLVM 21). nixpkgs'
              # assembled SDK carries no libc++ headers either.
              mkdir -p "$sdkDir/usr/include"
              for entry in ${sdkRoot}/*; do
                if [ "$(basename "$entry")" != usr ]; then
                  ln -s "$entry" "$sdkDir/"
                fi
              done
              for entry in ${sdkRoot}/usr/*; do
                if [ "$(basename "$entry")" != include ]; then
                  ln -s "$entry" "$sdkDir/usr/"
                fi
              done
              for entry in ${sdkRoot}/usr/include/*; do
                if [ "$(basename "$entry")" != c++ ]; then
                  ln -s "$entry" "$sdkDir/usr/include/"
                fi
              done
              ln -s "${sdkName}" "$sdkpath/MacOSX${lib.versions.major finalAttrs.version}.sdk"
              ln -s "${sdkName}" "$sdkpath/MacOSX.sdk"

              # Swift adds these locations to its search paths. Avoid spurious
              # warnings by making sure they exist, exactly as upstream does.
              mkdir -p "$platformPath/Developer/Library/Frameworks"
              mkdir -p "$platformPath/Developer/Library/PrivateFrameworks"
              mkdir -p "$platformPath/Developer/usr/lib"

              runHook postInstall
            '';

            passthru = {
              sdkroot = finalAttrs.finalPackage + "/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk";
            };

            __structuredAttrs = true;

            meta.description = "Pinned macOS SDK presented through the nixpkgs apple-sdk contract";
          })
    ) {};
    # Two adaptations over raw lld:
    #  - lld picks its driver from argv[0], and the bintools wrapper execs
    #    this file by absolute path, so a plain symlink named `ld` would
    #    select the ELF driver; re-exec under the ld64.lld name to force
    #    the Mach-O driver.
    #  - clang emits the legacy ld64 version protocol (`-macosx_version_min`
    #    plus a wrapper-injected `-sdk_version`) unless it believes the
    #    linker is modern, and ld64.lld dropped the legacy spelling ("must
    #    specify -platform_version"). Telling clang otherwise would leak
    #    `-mlinker-version` into compile-only invocations (an unused-argument
    #    warning under -Werror), so implement the legacy protocol here the
    #    way classic ld64 did: fold both flags into `-platform_version`.
    ld64 = prev.runCommand "ld64-lld-${llvmPkgs.lld.version}" {__structuredAttrs = true;} ''
      mkdir -p "$out/bin"
      cat > "$out/bin/ld" <<'EOF'
      #!${prev.runtimeShell}
      args=()
      min_version=""
      sdk_version=""
      while [ "$#" -gt 0 ]; do
        case $1 in
        -macosx_version_min)
          min_version=$2
          shift 2
          ;;
        -sdk_version)
          sdk_version=$2
          shift 2
          ;;
        *)
          args+=("$1")
          shift
          ;;
        esac
      done
      if [ -n "$min_version" ]; then
        args+=(-platform_version macos "$min_version" "''${sdk_version:-$min_version}")
      fi
      # fixDarwinDylibNames rewrites every dylib's install name to its
      # absolute store path after linking, and llvm-install-name-tool
      # silently corrupts the file when that growth outruns the header
      # padding (cctools' tool refuses instead). Always reserve the
      # maximum pad, as Xcode does.
      args+=(-headerpad_max_install_names)
      exec -a ld64.lld ${lib.getExe' llvmPkgs.lld "lld"} "''${args[@]}"
      EOF
      chmod +x "$out/bin/ld"
    '';
    # The wrapper build probes `ld -z now` and brands relro/bindnow
    # unsupported when the output matches "unknown option"; ld64.lld reports
    # "unknown argument", so the probe misses and every link would carry GNU
    # `-z` flags the Mach-O driver rejects. Brand them unsupported the same
    # way the wrapper itself does for AVR and Windows targets.
    bintools =
      if prev.stdenv.targetPlatform.isDarwin
      then
        prev.bintools.overrideAttrs (old: {
          postFixup =
            (old.postFixup or "")
            + ''
              substituteInPlace "$out/nix-support/add-hardening.sh" \
                --replace-fail "for flag in ; do" "for flag in relro bindnow; do"
              # The ld wrapper defaults to passing the whole command via a
              # bash-quoted @response file, which would smuggle the legacy
              # version flags past the ld64 shim's translation (see the ld64
              # shim above). Hand the argv over plainly instead.
              substituteInPlace "$out/bin/"*-ld \
                --replace-fail 'NIX_LD_USE_RESPONSE_FILE:-1' 'NIX_LD_USE_RESPONSE_FILE:-0'
            '';
        })
      else prev.bintools;
    cctools =
      prev.runCommand "cctools-llvm-${llvmPkgs.llvm.version}" {
        # `darwin.binutils-unwrapped` reads `cctools.version` and links the
        # `libtool` output; mirror upstream cctools' output layout so both
        # keep working.
        outputs = ["out" "dev" "man" "gas" "libtool"];
        version = llvmPkgs.llvm.version;
      } ''
        llvmbin=${lib.getBin llvmPkgs.llvm}/bin
        mkdir -p "$out/bin" "$dev" "$man" "$gas" "$libtool/bin"
        ln -s "$llvmbin/llvm-ranlib" "$out/bin/${targetPrefix}ranlib"
        ln -s "$llvmbin/llvm-lipo" "$out/bin/${targetPrefix}lipo"
        ln -s "$llvmbin/llvm-install-name-tool" "$out/bin/${targetPrefix}install_name_tool"
        ln -s "$llvmbin/llvm-libtool-darwin" "$libtool/bin/${targetPrefix}libtool"
        ${lib.optionalString (targetPrefix != "") ''
          ln -s "$llvmbin/llvm-libtool-darwin" "$libtool/bin/libtool"
        ''}
      '';
    # tcl's configure derives its platform branch from the build machine's
    # `uname` even when cross-compiling, so it configures a Linux build and
    # every subsequent link test fails. Seed the autoconf cache with the
    # Darwin answer (kernel version matching the SDK's macOS release).
    tcl =
      if prev.stdenv.hostPlatform.isDarwin
      then
        prev.tcl.overrideAttrs (old: {
          env = (old.env or {}) // {tcl_cv_sys_version = "Darwin-24.4.0";};
        })
      else prev.tcl;
    # Apple's libiconv port reexports libcharset (LC_REEXPORT_DYLIB), a load
    # command llvm-install-name-tool refuses to rewrite ("unsupported load
    # command"), which aborts meson's install-time rpath cleanup. GNU
    # libiconv has no reexports and served as nixpkgs' darwin iconv for
    # years, so the scope uses it instead.
    libiconv =
      if prev.stdenv.hostPlatform.isDarwin
      then prev.libiconvReal
      else prev.libiconv;
  };
in
  import pkgs.path {
    localSystem = pkgs.stdenv.hostPlatform.system;
    crossSystem = {
      config = target;
      # Align both platform versions with the SDK actually in the scope:
      # nixpkgs' defaults pair a 14.x SDK with a 14.0 floor, and building
      # against the 15.4 SDK with an older floor trips
      # `-Werror=unguarded-availability` the moment configure detects a
      # symbol the SDK declares as newer (bash: strchrnul). The produced
      # closure assumes macOS >= this version at runtime.
      darwinSdkVersion = sdkPin.version;
      darwinMinVersion = sdkPin.version;
    };
    config = {
      # Darwin-only `meta.platforms` still gates the linux-hosted copies of
      # apple packages this scope keeps (tapi, xar, ...), which build fine
      # on linux but refuse to evaluate without the escape hatch.
      allowUnsupportedSystem = true;
      # The native darwin stdenv ships fixDarwinDylibNames in its extra
      # native build inputs; stdenv/cross does not, so every cross-built
      # dylib would keep whatever install name its build system emitted
      # (sqlite: a bare `libsqlite3.dylib`) and consumers would record an
      # unresolvable load command. Mirror the native stdenv.
      replaceCrossStdenv = {
        buildPackages,
        baseStdenv,
      }:
        baseStdenv.override (old: {
          extraNativeBuildInputs = old.extraNativeBuildInputs ++ [buildPackages.fixDarwinDylibNames];
        });
    };
    overlays = [shims];
  }
