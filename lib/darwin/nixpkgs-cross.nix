# Linux -> Darwin nixpkgs cross scope for C/C++ closures that build through
# upstream nixpkgs packaging instead of the cargo-unit lane (RFC 0009). The
# consumer today is `nix-ix`: packages/nix/nix swaps its component scope to
# this one when the registry cross lane instantiates it, so the full modular
# nix closure builds on the linux fleet and a Mac only substitutes (#3585).
#
# Upstream declares Linux-hosted Darwin cross unsupported (the nixpkgs manual
# platform table; NixOS/nixpkgs#405893 keeps deferring the port), and exactly
# three `pkgsBuildHost` packages prove it by compiling Apple-only source with
# the linux gcc: `ld64` (needs mach/dispatch/uuid headers), the `cctools`
# suite (blocked behind its ld64 dependency), and the SDK's source-built
# `libresolv` (BSD cdefs and headers). Everything else in the toolchain
# closure builds on linux. So this scope is stock nixpkgs cross plus one
# overlay replacing those three with parts that do build there:
#
#   ld64      -> lld's `ld64.lld` Mach-O driver. `darwin.binutils-unwrapped`
#                consumes plain `${ld64}/bin/ld` and keeps its cctools-shaped
#                wrapper handling, so the swap is invisible downstream, and
#                ld64.lld ad-hoc-signs arm64 output on its own.
#   cctools   -> the llvm equivalents of the tools the binutils assembly and
#                the SDK's xcrun propagation actually take from cctools
#                (ranlib, lipo, install_name_tool, libtool; nm/otool/strip
#                already come from llvm there). Tools with no llvm equivalent
#                (codesign_allocate, gprof) are omitted: both consumers link
#                tools behind existence checks and nothing in this lane
#                invokes them.
#   libresolv -> headers plus `.tbd` link stubs lifted from the pinned macOS
#                SDK (`ix.macosSdk`, same licensing posture as the rust
#                lane). The nixpkgs SDK assembly only consumes libresolv
#                headers (cups-headers' configure) and propagates it for
#                link stubs; consumers bind /usr/lib/libresolv.9.dylib at
#                runtime on a real Mac.
#
# The overlay applies to every stage of the scope, including darwin-hosted
# `targetPackages` copies nothing in this lane ever executes; that keeps the
# replacement total instead of special-casing stages.
{macosSdk}: {
  pkgs,
  target,
}: let
  sdkRoot = macosSdk {inherit pkgs;};
  shims = _final: prev: let
    inherit (prev) lib;
    llvmPkgs = prev.llvmPackages;
    inherit (prev.stdenv) hostPlatform targetPlatform;
    targetPrefix = lib.optionalString (targetPlatform != hostPlatform) "${targetPlatform.config}-";
  in {
    ld64 = prev.runCommand "ld64-lld-${llvmPkgs.lld.version}" {} ''
      mkdir -p "$out/bin"
      ln -s ${lib.getExe' llvmPkgs.lld "ld64.lld"} "$out/bin/ld"
    '';
    cctools =
      prev.runCommand "cctools-llvm-${llvmPkgs.llvm.version}" {
        # `darwin.binutils-unwrapped` reads `cctools.version`, and the SDK's
        # xcrun propagation reads the `libtool` output; mirror upstream
        # cctools' output layout so both keep working.
        outputs = ["out" "dev" "man" "gas" "libtool"];
        version = llvmPkgs.llvm.version;
      } ''
        llvmbin=${lib.getBin llvmPkgs.llvm}/bin
        mkdir -p "$out/bin" "$dev" "$man" "$gas" "$libtool/bin"
        ln -s "$llvmbin/llvm-ranlib" "$out/bin/${targetPrefix}ranlib"
        ln -s "$llvmbin/llvm-lipo" "$out/bin/${targetPrefix}lipo"
        ln -s "$llvmbin/llvm-install-name-tool" "$out/bin/${targetPrefix}install_name_tool"
        ln -s "$llvmbin/llvm-libtool-darwin" "$libtool/bin/${targetPrefix}libtool"
        ln -s "$llvmbin/llvm-libtool-darwin" "$libtool/bin/libtool"
      '';
    darwin = prev.darwin.overrideScope (_dfinal: _dprev: {
      libresolv =
        prev.runCommand "libresolv-sdk-stub" {
          # Mirror upstream libresolv's outputs; `dev` is what cups-headers'
          # configure and the SDK header propagation consume.
          outputs = ["out" "dev" "man"];
        } ''
          mkdir -p "$out/lib" "$dev/include/arpa" "$man"
          cp ${sdkRoot}/usr/lib/libresolv.tbd ${sdkRoot}/usr/lib/libresolv.9.tbd "$out/lib/"
          for header in resolv.h dns.h dns_sd.h dns_util.h nameser.h; do
            cp "${sdkRoot}/usr/include/$header" "$dev/include/"
          done
          cp ${sdkRoot}/usr/include/arpa/nameser.h ${sdkRoot}/usr/include/arpa/nameser_compat.h "$dev/include/arpa/"
        '';
    });
  };
in
  import pkgs.path {
    localSystem = pkgs.stdenv.hostPlatform.system;
    crossSystem = {config = target;};
    # Darwin-only `meta.platforms` still gates the linux-hosted copies of
    # SDK source packages this scope does not shim (libsbuf, libutil,
    # copyfile, ...), which build fine on linux but refuse to evaluate
    # without the escape hatch.
    config.allowUnsupportedSystem = true;
    overlays = [shims];
  }
