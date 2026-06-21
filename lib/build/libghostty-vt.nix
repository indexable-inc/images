{
  lib,
  writeNushellApplication,
}:

/**
  Build libghostty-vt: ghostty's terminal VT engine as a standalone C library.

  Ghostty's `build.zig` exposes a VT-only artifact through `-Demit-lib-vt=true`,
  which skips the GUI app, xcframework, and docs and emits just the parser,
  screen model, and render-state API. The result is a static `libghostty-vt.a`
  plus a self-contained `libghostty-vt.<ver>.dylib`/`.so`, the `ghostty/` C
  headers, and a pkg-config file.

  Arguments:
  - `pkgs`: package set to build against; the artifact is host-system specific.
  - `ghosttySource`: ghostty source tree (the `ghostty` flake input). Must ship
    `build.zig`, `build.zig.zon`, and `build.zig.zon.nix` (the zon2nix output
    that vendors every lazy Zig dependency with SRI hashes for a network-free
    build).
  - `version`: derivation version. Defaults to the value in `build.zig.zon`.

  The static archive does not bundle its C++ dependencies (`libhighway`,
  `libsimdutf`, `libutfcpp`) and needs `-lc++`; the dylib is self-contained, so
  `ix-vt-sys` links the dylib to avoid that archive dance.
*/
pkgs:
{
  ghosttySource,
  version ? "1.3.2-dev",
}:
let
  inherit (pkgs) stdenv;

  # zon2nix output checked into the ghostty tree. It materializes a link farm of
  # `<zig-hash> -> source` for every dependency, populated into the Zig global
  # cache `p/` directory below so `zig build` resolves deps offline.
  deps = pkgs.callPackage (ghosttySource + "/build.zig.zon.nix") {
    inherit (pkgs) zig_0_15;
    name = "libghostty-vt-deps-${version}";
  };

  isDarwin = stdenv.hostPlatform.isDarwin;

  # The apple-sdk that zig links against inside the sandbox. zig 0.15.2 finds
  # the SDK by shelling out to `xcode-select` / `xcrun`, which do not exist in
  # the Nix build sandbox, so the SDK detection is shimmed below to point at
  # this pinned SDK rather than a host install.
  appleSdk = pkgs.apple-sdk_14;
  appleSdkRoot = "${appleSdk}/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk";

  # Shim zig's darwin SDK probe. ghostty's vendored C/C++ deps (`zlib`,
  # `simdutf`, `highway`, `utfcpp`) call `apple_sdk.addPaths`, which runs
  # `std.zig.system.darwin.getSdk` -> `xcrun --show-sdk-path` and
  # `isSdkInstalled` -> `xcode-select --print-path`. Neither tool is in the
  # sandbox, so without these shims the build crashes with `DarwinSdkNotFound`.
  # Returning the pinned Nix SDK path keeps the build hermetic.
  # `--wrapped main [...args]` lets the shim swallow every flag zig passes
  # (`xcrun --sdk macosx --show-sdk-path`, `xcode-select --print-path`) and just
  # echo the pinned path. writeShellScriptBin is banned (no nu-check, no declared
  # deps), so these go through the checked Nushell writer.
  xcrunShim = writeNushellApplication pkgs {
    name = "xcrun";
    text = ''
      # nu
      def --wrapped main [...args] { print "${appleSdkRoot}" }
    '';
  };
  xcodeSelectShim = writeNushellApplication pkgs {
    name = "xcode-select";
    text = ''
      # nu
      def --wrapped main [...args] { print "${appleSdk}" }
    '';
  };

  darwinSdkInputs = lib.optionals isDarwin [
    xcrunShim
    xcodeSelectShim
  ];
in
stdenv.mkDerivation {
  pname = "libghostty-vt";
  inherit version;

  src = builtins.path {
    name = "ghostty-source";
    path = ghosttySource;
  };

  strictDeps = true;

  nativeBuildInputs = [
    pkgs.zig_0_15
    pkgs.pkg-config
  ]
  ++ darwinSdkInputs;

  # `zlib` is consumed through `-fsys=zlib` so ghostty's framegen tool links the
  # nixpkgs zlib instead of building the vendored copy (which would re-trigger
  # the apple-sdk probe). `apple-sdk` is a buildInput on darwin so the linker
  # resolves the system frameworks the C++ deps pull in.
  buildInputs = [
    pkgs.zlib
  ]
  ++ lib.optional isDarwin appleSdk;

  dontConfigure = true;
  dontBuild = true;

  installPhase = ''
    # shell
    runHook preInstall

    export ZIG_GLOBAL_CACHE_DIR="$TMPDIR/zig-cache"
    mkdir -p "$ZIG_GLOBAL_CACHE_DIR/p"
    cp -R --no-preserve=mode ${deps}/. "$ZIG_GLOBAL_CACHE_DIR/p/"

    ${lib.optionalString isDarwin ''
      export SDKROOT="${appleSdkRoot}"
      export DEVELOPER_DIR="${appleSdk}"
    ''}

    buildCores=1
    if [ "''${enableParallelBuilding-1}" ]; then
      buildCores="$NIX_BUILD_CORES"
    fi

    zig build \
      "-j$buildCores" \
      --global-cache-dir "$ZIG_GLOBAL_CACHE_DIR" \
      --cache-dir "$TMPDIR/zig-local-cache" \
      -Demit-lib-vt=true \
      -Dcpu=baseline \
      -Doptimize=ReleaseFast \
      -fsys=zlib --search-prefix ${pkgs.zlib} \
      --prefix "$out" \
      --summary all

    runHook postInstall
  '';

  doCheck = false;

  meta = {
    description = "Ghostty's terminal VT engine as a standalone C library (parser, screen, render state)";
    homepage = "https://ghostty.org/";
    license = lib.licenses.mit;
    platforms = lib.platforms.unix;
  };
}
