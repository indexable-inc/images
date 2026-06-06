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
# `writeNushellApplication` is the repo writer from lib/util/writers.nix; the
# wrappers below are Nu because writeShellApplication is lint-banned.
{
  lib,
  pkgs,
  mcp,
  writeNushellApplication,
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

  codexWithIndexMcp = writeNushellApplication pkgs {
    name = "codex-with-index-mcp";
    runtimeInputs = [
      pkgs.coreutils
      pkgs.codex
    ];
    text = ''
      def --wrapped main [...args] {
        let source_home = $env.ROOM_CODEX_AUTH_HOME? | default (
          $env.CODEX_HOME? | default ($env.HOME | path join ".codex")
        )
        let runtime_root = $env.XDG_RUNTIME_DIR? | default (
          $env.TMPDIR? | default "/tmp"
        ) | path join "symphony-codex"
        mkdir $runtime_root
        let isolated_home = mktemp --directory --tmpdir-path $runtime_root "codex-home.XXXXXX"

        let source_auth = $source_home | path join "auth.json"
        if ($source_auth | path exists) {
          ^ln -s $source_auth ($isolated_home | path join "auth.json")
        }

        # codex churns config.toml at runtime, so copy the generated file in
        # writable (a /nix/store copy is 0444).
        ^install -m600 ${codexConfig} ($isolated_home | path join "config.toml")

        $env.CODEX_HOME = $isolated_home
        exec ${lib.getExe pkgs.codex} ...$args
      }
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
          --prefix PATH : ${
            lib.makeBinPath [
              codexWithIndexMcp
              pkgs.codex
            ]
          } \
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
  tauriDev = writeNushellApplication pkgs {
    name = "tauri-dev";
    runtimeInputs = [
      pkgs.coreutils
      pkgs.git
      pkgs.nodejs
      rustToolchain
    ];
    text = ''
      def main [] {
        let repo_root = do { ^git rev-parse --show-toplevel } | complete
        if $repo_root.exit_code != 0 {
          error make { msg: "tauri-dev: run from inside the index checkout" }
        }
        cd ($repo_root.stdout | str trim | path join "packages/symphony/packages/room")
        if not ("node_modules" | path exists) {
          ^npm ci
        }
        exec npm run "tauri:dev"
      }
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
    room = ./modules/services/room.nix;
    symphony = ./modules/services/symphony.nix;
  };
}
