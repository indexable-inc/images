{
  lib,
  stdenv,
  stdenvNoCC,
  rustPlatform,
  cargo-tauri,
  bun,
  nodejs,
  cargo,
  rustc,
  curl,
  cacert,
  fetchurl,
  makeWrapper,
  writableTmpDirAsHomeHook,
}:
let
  # Pixel-accurate Minecraft "Mojangles" TTF generated from the real Minecraft
  # font definitions (tryashtar/minecraft-ttf), so the boss bar title renders in
  # the exact proportional font Minecraft draws. Like the boss bar sprites above,
  # this is Mojang-derived art: fetched at build time and NOT redistributed by
  # this repo. It is intentionally not an OFL lookalike; the operator chose a
  # true 1:1 font over a pure-OFL substitute.
  minecraftFont = fetchurl {
    url = "https://github.com/tryashtar/minecraft-ttf/releases/download/v1.4/MinecraftDefault-Regular.ttf";
    hash = "sha256-/DH9yXRU/qFDJacCuoJOWwtQRsADevs8oFWgPircqSA=";
  };

  # Vanilla Minecraft boss bar sprite textures the overlay renders. These are
  # Mojang's art, gitignored and NOT redistributed in this repo; the fetcher
  # pulls them at build time, pinned to a Minecraft version, from the
  # InventivetalentDev/minecraft-assets mirror. This mirrors the offline-only
  # `scripts/fetch-assets.sh` so the `prebuild` hook no-ops against pre-seeded
  # files.
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

  bossBarSprites = stdenvNoCC.mkDerivation {
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

  # Frontend dependencies, resolved from the tracked `bun.lock`. A fixed-output
  # derivation so the rest of the build stays offline.
  node_modules = stdenvNoCC.mkDerivation {
    pname = "bossbar-overlay-node_modules";
    version = "0.1.0";

    src = lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.unions [
        ../package.json
        ../bun.lock
      ];
    };

    impureEnvVars = lib.fetchers.proxyImpureEnvVars ++ [
      "GIT_PROXY_COMMAND"
      "SOCKS_SERVER"
    ];

    nativeBuildInputs = [
      bun
      writableTmpDirAsHomeHook
    ];

    dontConfigure = true;

    buildPhase = ''
      runHook preBuild
      bun install \
        --frozen-lockfile \
        --no-progress \
        --ignore-scripts \
        --cpu="*" \
        --os="*"
      runHook postBuild
    '';

    installPhase = ''
      runHook preInstall
      mkdir -p $out
      cp -R node_modules $out/node_modules
      runHook postInstall
    '';

    dontFixup = true;

    outputHashMode = "recursive";
    outputHashAlgo = "sha256";
    outputHash = "sha256-97uhbMuDtHGPv9WIwcHzIQ8aEOheYUBKjCoVguPs8vo=";
  };
in
rustPlatform.buildRustPackage {
  pname = "bossbar-overlay";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../package.json
      ../bun.lock
      ../tsconfig.json
      ../vite.config.ts
      ../index.html
      ../src
      ../src-tauri
      ../scripts
    ];
  };

  cargoRoot = "src-tauri";
  buildAndTestSubdir = "src-tauri";
  cargoLock.lockFile = ../src-tauri/Cargo.lock;

  strictDeps = true;

  nativeBuildInputs = [
    cargo-tauri.hook
    bun
    nodejs
    cargo
    rustc
    makeWrapper
  ];

  # macOS uses the system WKWebView, so no gtk/webkit buildInputs are needed.
  # The Linux toolchain would require them; gate when that target lands.

  tauriBuildFlags = [ "--no-sign" ];

  preBuild = ''
    cp -a ${node_modules}/node_modules .
    chmod -R u+w node_modules
    patchShebangs node_modules

    # Pre-seed the gitignored Mojang sprites so the `prebuild` fetch-assets
    # hook finds them and stays offline.
    mkdir -p src/assets/boss_bar
    cp ${bossBarSprites}/*.png src/assets/boss_bar/
    chmod -R u+w src/assets/boss_bar

    # Pre-seed the Mojang-derived Minecraft TTF so `vite` bundles it offline.
    mkdir -p src/assets/fonts
    cp ${minecraftFont} src/assets/fonts/MinecraftDefault-Regular.ttf
    chmod -R u+w src/assets/fonts
  '';

  postInstall = lib.optionalString stdenv.hostPlatform.isDarwin ''
    mkdir -p $out/bin
    ln -s "$out/Applications/Boss Bar Overlay.app/Contents/MacOS/bossbar-overlay" $out/bin/bossbar-overlay
  '';

  meta = {
    description = "Minecraft-style boss bar desktop overlay driven by an SQLite file";
    # MIT (Copyright 2026 Indexable Inc.). The boss bar sprites are Mojang art
    # fetched at build time, not redistributed by this package.
    license = lib.licenses.mit;
    mainProgram = "bossbar-overlay";
    platforms = [
      "aarch64-darwin"
      "x86_64-darwin"
    ];
  };
}
