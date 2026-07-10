{
  lib,
  stdenv,
  rustPlatform,
  makeWrapper,
  pkg-config,
  # The Minecraft art (boss bar sprites, book texture, bitmap font) extracted from
  # Mojang's official client jar by a reproducible Nix derivation.
  minecraft-assets,
  # Linux runtime libraries. winit/layer-shell dlopen or link
  # X11/Wayland/xkbcommon and wgpu dlopens the Vulkan loader, so they must be on
  # the runtime library path.
  wayland,
  libxkbcommon,
  vulkan-loader,
  libGL,
  xorg,
}:
# Builds the whole desktop-overlay workspace: `bossbar-overlay` and `book-overlay`
# share the `overlay-core` float + wgpu engine and are produced from one
# `Cargo.lock`. The Mojang art is dropped in from `minecraft-assets` before
# compiling (the source closure carries only code), so nothing here trusts a
# third-party mirror. `meta.mainProgram` is `bossbar-overlay`; the `book-overlay`
# flake output is this same build with its main program overridden.
let
  # winit + wgpu need these at runtime on Linux; empty on Darwin (Metal + the
  # system window server).
  runtimeLibs = lib.optionals stdenv.hostPlatform.isLinux [
    wayland
    libxkbcommon
    vulkan-loader
    libGL
    xorg.libX11
    xorg.libXcursor
    xorg.libXrandr
    xorg.libXi
  ];
  bins = [
    "bossbar-overlay"
    "book-overlay"
    "xp-orb-overlay"
  ];
in
  rustPlatform.buildRustPackage {
    pname = "minecraft-overlays";
    version = "0.1.0";

    # Only the code: the gitignored Mojang art under each crate's `assets/` is left
    # out and supplied by `minecraft-assets` in preBuild.
    src = lib.fileset.toSource {
      root = ./.;
      fileset = lib.fileset.unions [
        ./Cargo.toml
        ./Cargo.lock
        ./crates/overlay-core/Cargo.toml
        ./crates/overlay-core/src
        ./crates/bossbar/Cargo.toml
        ./crates/bossbar/src
        ./crates/book/Cargo.toml
        ./crates/book/src
        ./crates/orb/Cargo.toml
        ./crates/orb/src
      ];
    };

    cargoLock.lockFile = ./Cargo.lock;

    strictDeps = true;

    nativeBuildInputs = [makeWrapper] ++ lib.optional stdenv.hostPlatform.isLinux pkg-config;
    buildInputs = runtimeLibs;

    preBuild = ''
      # shell
      # Drop the extracted Minecraft art where each crate's `include_bytes!` expects
      # it: the shared font into overlay-core, the sprites into each app.
      mkdir -p crates/overlay-core/assets crates/bossbar/assets/boss_bar \
        crates/book/assets/gui crates/orb/assets/entity crates/orb/assets/particle
      cp ${minecraft-assets}/font/ascii.png crates/overlay-core/assets/ascii.png
      cp ${minecraft-assets}/boss_bar/*.png crates/bossbar/assets/boss_bar/
      cp ${minecraft-assets}/gui/*.png crates/book/assets/gui/
      cp ${minecraft-assets}/entity/experience_orb.png crates/orb/assets/entity/
      cp ${minecraft-assets}/particle/angry.png crates/orb/assets/particle/
      chmod -R u+w crates
    '';

    # Point each binary at the dlopened X11/Wayland/Vulkan libraries on Linux.
    postFixup = lib.optionalString stdenv.hostPlatform.isLinux (
      lib.concatMapStringsSep "\n" (b: ''
        patchelf --add-rpath "${lib.makeLibraryPath runtimeLibs}" "$out/bin/${b}"
        wrapProgram "$out/bin/${b}" \
          --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath runtimeLibs}"
      '')
      bins
    );

    meta = {
      description = "Minecraft-style desktop overlays (boss bar + book), wgpu, SQLite-driven";
      # MIT (Copyright 2026 Indexable Inc.). The Minecraft textures and font are
      # Mojang(-derived) art extracted at build time, not redistributed here.
      license = lib.licenses.mit;
      mainProgram = "bossbar-overlay";
      platforms = [
        "aarch64-darwin"
        "x86_64-darwin"
        "x86_64-linux"
        "aarch64-linux"
      ];
    };
  }
