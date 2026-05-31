{
  lib,
  stdenv,
  rustPlatform,
  fetchurl,
  curl,
  cacert,
  makeWrapper,
  # Linux runtime libraries. winit dlopens X11/Wayland/xkbcommon and wgpu
  # dlopens the Vulkan loader, so they must be on the runtime library path.
  wayland,
  libxkbcommon,
  vulkan-loader,
  libGL,
  xorg,
}:
let
  # Pixel-accurate Minecraft "Mojangles" TTF generated from the real Minecraft
  # font definitions (tryashtar/minecraft-ttf), so boss bar titles render in the
  # exact proportional font Minecraft draws. Mojang-derived art: fetched at
  # build time and NOT redistributed by this repo. It is intentionally not an
  # OFL lookalike; the operator chose a true 1:1 font over a pure-OFL substitute.
  minecraftFont = fetchurl {
    url = "https://github.com/tryashtar/minecraft-ttf/releases/download/v1.4/MinecraftDefault-Regular.ttf";
    hash = "sha256-/DH9yXRU/qFDJacCuoJOWwtQRsADevs8oFWgPircqSA=";
  };

  # Vanilla Minecraft boss bar sprite textures the overlay renders. Mojang's
  # art, gitignored and NOT redistributed in this repo; the fetcher pulls them
  # at build time, pinned to a Minecraft version, from the
  # InventivetalentDev/minecraft-assets mirror. This mirrors the offline
  # `app/scripts/fetch-assets.sh` used for local development.
  minecraftVersion = "1.21";
  spriteColors = [
    "pink"
    "blue"
    "red"
    "green"
    "yellow"
    "purple"
    "white"
  ];
  spriteNotches = [
    "notched_6"
    "notched_10"
    "notched_12"
    "notched_20"
  ];
  spriteNames = lib.concatMap (base: [
    "${base}_background.png"
    "${base}_progress.png"
  ]) (spriteColors ++ spriteNotches);

  bossBarSprites = stdenv.mkDerivation {
    pname = "bossbar-overlay-sprites";
    version = minecraftVersion;

    dontUnpack = true;
    strictDeps = true;
    nativeBuildInputs = [
      curl
      cacert
    ];

    buildPhase = ''
      runHook preBuild
      base="https://raw.githubusercontent.com/InventivetalentDev/minecraft-assets/${minecraftVersion}/assets/minecraft/textures/gui/sprites/boss_bar"
      mkdir -p "$out"
      for name in ${lib.concatStringsSep " " spriteNames}; do
        curl -fsSL -o "$out/$name" "$base/$name"
      done
      runHook postBuild
    '';

    dontInstall = true;
    dontFixup = true;

    outputHashMode = "recursive";
    outputHashAlgo = "sha256";
    outputHash = "sha256-XPq8Ik6YUiwVyy2wXWR9V2wxi1TwyudVwdB0IuQLk50=";
  };

  # winit + wgpu need these at runtime on Linux; empty on Darwin, which uses
  # Metal and the system window server.
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
in
rustPlatform.buildRustPackage {
  pname = "bossbar-overlay";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./src
    ];
  };

  cargoLock.lockFile = ./Cargo.lock;

  strictDeps = true;

  nativeBuildInputs = [ makeWrapper ];
  buildInputs = runtimeLibs;

  preBuild = ''
    # The Mojang sprites and TTF are gitignored, so the source closure never
    # carries them; drop them where `include_bytes!` expects before compiling.
    mkdir -p assets/boss_bar assets/fonts
    cp ${bossBarSprites}/*.png assets/boss_bar/
    cp ${minecraftFont} assets/fonts/MinecraftDefault-Regular.ttf
    chmod -R u+w assets
  '';

  # Point the binary at the dlopened X11/Wayland/Vulkan libraries on Linux.
  postFixup = lib.optionalString stdenv.hostPlatform.isLinux ''
    patchelf --add-rpath "${lib.makeLibraryPath runtimeLibs}" "$out/bin/bossbar-overlay"
    wrapProgram "$out/bin/bossbar-overlay" \
      --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath runtimeLibs}"
  '';

  meta = {
    description = "Minecraft-style boss bar desktop overlay driven by an SQLite file";
    # MIT (Copyright 2026 Indexable Inc.). The boss bar sprites and the
    # Minecraft TTF are Mojang(-derived) art fetched at build time, not
    # redistributed by this package.
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
