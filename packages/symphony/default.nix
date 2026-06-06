# Symphony's package surface, lifted from its former standalone flake when the
# tree was tucked into index. Returning `packages` and `nixosModules` mirrors a
# flake's output shape so the caller in `lib/overlay.nix` can read values out
# without learning a new convention.
#
# The caller supplies a `pkgs` that already has rust-overlay applied
# (room-server pins a nightly toolchain by date), plus the resolved index `mcp`
# derivation the codex wrapper spawns as the agent's only MCP server. Passing
# `mcp` in (instead of importing index's lib here) keeps the dependency
# direction one-way and avoids a circular reference through the overlay.
{
  lib,
  pkgs,
  mcp,
}:
let
  rustToolchain = pkgs.rust-bin.nightly."2026-05-04".default;
  rustPlatform = pkgs.makeRustPlatform {
    cargo = rustToolchain;
    rustc = rustToolchain;
  };

  roomSiteSrc = lib.fileset.toSource {
    root = ./packages/room;
    fileset = lib.fileset.unions [
      ./packages/room/index.html
      ./packages/room/package.json
      ./packages/room/package-lock.json
      ./packages/room/public
      ./packages/room/src
      ./packages/room/tsconfig.json
      ./packages/room/vite.config.ts
    ];
  };

  rustWorkspaceSrc = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./Cargo.nix.lock
      ./packages/room-server/Cargo.toml
      ./packages/room-server/src
      ./packages/room-server/tests
      # engine_contract test reads ../../contracts/fixtures/*.json during the
      # build's checkPhase, so the fixtures must be in the closure.
      ./contracts/fixtures
    ];
  };

  # room-server shells out to `codex app-server` over JSON-RPC. The wrapper
  # pins Codex and gives it an isolated config whose only MCP server is the
  # index MCP passed in by the caller. Locks codex to that surface: every
  # built-in tool codex 0.135 exposes a switch for is off, so the agent works
  # exclusively through the index (Jupyter/python + search) surface.
  # apply_patch and the plan tool have no toggle yet (openai/codex#6049), so
  # those two remain.
  codexConfig = (pkgs.formats.toml { }).generate "codex-index-only.toml" {
    web_search = "disabled";
    features = {
      shell_tool = false;
      unified_exec = false;
      browser_use = false;
      browser_use_external = false;
      in_app_browser = false;
      computer_use = false;
      image_generation = false;
      multi_agent = false;
      apps = false;
      plugins = false;
      plugin_sharing = false;
      hooks = false;
      goals = false;
    };
    mcp_servers.index.command = lib.getExe mcp;
  };

  codexWithIndexMcp = pkgs.writeShellApplication {
    name = "codex-with-index-mcp";
    runtimeInputs = [
      pkgs.coreutils
      pkgs.codex
    ];
    text = ''
      source_home="''${ROOM_CODEX_AUTH_HOME:-''${CODEX_HOME:-$HOME/.codex}}"
      runtime_root="''${XDG_RUNTIME_DIR:-''${TMPDIR:-/tmp}}/symphony-codex"
      mkdir -p "$runtime_root"
      isolated_home="$(mktemp -d "$runtime_root/codex-home.XXXXXX")"

      if [ -f "$source_home/auth.json" ]; then
        ln -s "$source_home/auth.json" "$isolated_home/auth.json"
      fi

      # codex churns config.toml at runtime, so copy the generated file in
      # writable (a /nix/store copy is 0444).
      install -m600 ${codexConfig} "$isolated_home/config.toml"

      export CODEX_HOME="$isolated_home"
      exec ${lib.getExe pkgs.codex} "$@"
    '';
  };

  roomServerRaw = rustPlatform.buildRustPackage {
    pname = "room-server";
    version = "0.1.0";
    src = rustWorkspaceSrc;
    cargoLock = {
      lockFile = ./Cargo.nix.lock;
    };
    cargoBuildFlags = [
      "-p"
      "room-server"
    ];
    cargoTestFlags = [
      "-p"
      "room-server"
    ];
    strictDeps = true;
    meta.mainProgram = "room-server";
  };

  roomServer =
    pkgs.runCommand "room-server-wrapped"
      {
        nativeBuildInputs = [ pkgs.makeWrapper ];
        meta = (roomServerRaw.meta or { }) // {
          mainProgram = "room-server";
        };
      }
      ''
        mkdir -p $out/bin
        makeWrapper ${roomServerRaw}/bin/room-server $out/bin/room-server \
          --prefix PATH : ${lib.makeBinPath [ codexWithIndexMcp pkgs.codex ]} \
          --set-default ROOM_CODEX_BIN ${lib.getExe codexWithIndexMcp}
      '';

  roomSite = pkgs.buildNpmPackage {
    pname = "room-site";
    version = "0.1.0";
    src = roomSiteSrc;
    npmDeps = pkgs.importNpmLock {
      npmRoot = roomSiteSrc;
    };
    npmConfigHook = pkgs.importNpmLock.npmConfigHook;
    strictDeps = true;
    buildPhase = ''
      runHook preBuild
      PATH="$PWD/node_modules/.bin:$PATH"
      command -v vite
      npm run build
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      cp -R dist $out
      runHook postInstall
    '';
  };

  # Launcher for the Tauri desktop client. The client is not Nix-built (WebKit,
  # codesign, and bundle formats are out of scope), so this only supplies the
  # node + rust toolchain, cds into the live working tree's room subdir, and
  # execs `tauri dev`. It operates on the checkout, not the store copy, because
  # `tauri dev` writes node_modules, target/, and gen/ in place.
  tauriDev = pkgs.writeShellApplication {
    name = "tauri-dev";
    runtimeInputs = [
      pkgs.coreutils
      pkgs.git
      pkgs.nodejs
      rustToolchain
    ];
    text = ''
      repo_root="$(git -C "$PWD" rev-parse --show-toplevel 2>/dev/null || true)"
      if [ -z "$repo_root" ]; then
        echo "tauri-dev: run from inside the index checkout" >&2
        exit 1
      fi
      cd "$repo_root/packages/symphony/packages/room"
      if [ ! -d node_modules ]; then
        npm ci
      fi
      exec npm run tauri:dev
    '';
  };
in
{
  packages = {
    room-server = roomServer;
    room-site = roomSite;
    tauri-dev = tauriDev;
  };

  nixosModules = {
    room = ./modules/services/room;
    symphony = ./modules/services/symphony;
  };
}
