{
  description = "Symphony: an Elixir runtime for .sym agent workflows";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    index = {
      url = "github:indexable-inc/index";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay, index }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f:
        nixpkgs.lib.genAttrs systems (system:
          f (import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          }));
      inherit (nixpkgs) lib;

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
          # engine_contract test reads ../../contracts/fixtures/*.json during
          # the build's checkPhase, so the fixtures must be in the closure.
          ./contracts/fixtures
        ];
      };

      # room-server shells out to `codex app-server` over JSON-RPC.
      # The wrapper pins Codex and gives it an isolated config whose
      # only MCP server is the index MCP from the index flake input.
      codexWithIndexMcp = pkgs:
        let
          ixMcp = index.packages.${pkgs.stdenv.hostPlatform.system}.mcp;
          # Codex config built declaratively from an attrset (not hand-written
          # TOML) so the table/scalar layout is always valid. Locks codex to the
          # index MCP only: every built-in tool codex 0.135 exposes a switch for
          # is off, so the agent works exclusively through the index
          # (Jupyter/python + search) surface. apply_patch and the plan tool have
          # no toggle yet (openai/codex#6049), so those two remain.
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
            mcp_servers.index.command = lib.getExe ixMcp;
          };
        in
        pkgs.writeShellApplication {
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

      wrapRoomServerWithCodex = pkgs: raw:
        let
          codex = codexWithIndexMcp pkgs;
        in
        pkgs.runCommand "room-server-wrapped"
          {
            nativeBuildInputs = [ pkgs.makeWrapper ];
            meta = (raw.meta or { }) // { mainProgram = "room-server"; };
          }
          ''
            mkdir -p $out/bin
            makeWrapper ${raw}/bin/room-server $out/bin/room-server \
              --prefix PATH : ${lib.makeBinPath [ codex pkgs.codex ]} \
              --set-default ROOM_CODEX_BIN ${lib.getExe codex}
          '';
    in {
      packages = forAllSystems (pkgs:
        let
          rustToolchain = pkgs.rust-bin.nightly."2026-05-04".default;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rustToolchain;
            rustc = rustToolchain;
          };
          roomServerRaw = rustPlatform.buildRustPackage {
            pname = "room-server";
            version = "0.1.0";
            src = rustWorkspaceSrc;
            cargoLock = {
              lockFile = ./Cargo.nix.lock;
            };
            cargoBuildFlags = [ "-p" "room-server" ];
            cargoTestFlags = [ "-p" "room-server" ];
            meta.mainProgram = "room-server";
          };
          roomServer = wrapRoomServerWithCodex pkgs roomServerRaw;
          roomSite = pkgs.buildNpmPackage {
            pname = "room-site";
            version = "0.1.0";
            src = roomSiteSrc;
            npmDeps = pkgs.importNpmLock {
              npmRoot = roomSiteSrc;
            };
            npmConfigHook = pkgs.importNpmLock.npmConfigHook;
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

        in {
        default = pkgs.writeShellApplication {
          name = "symphony";
          runtimeInputs = [
            pkgs.bash
            pkgs.cacert
            pkgs.coreutils
            pkgs.elixir_1_19
            pkgs.erlang_28
            pkgs.gh
            pkgs.git
            pkgs.openssh
          ];
          text = ''
            exec ${self}/bin/run-nix "$@"
          '';
        };
        room-server = roomServer;
        room-site = roomSite;
        # Launcher for the Tauri desktop client. The client is not
        # Nix-built (WebKit, codesign, and bundle formats are out of
        # scope), so this only supplies the node + rust toolchain, cds
        # into the live working tree's packages/room, and execs `tauri
        # dev`. It operates on the checkout, not the store copy, because
        # `tauri dev` writes node_modules, target/, and gen/ in place.
        tauri-dev = pkgs.writeShellApplication {
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
              echo "tauri-dev: run from inside the symphony checkout" >&2
              exit 1
            fi
            cd "$repo_root/packages/room"
            if [ ! -d node_modules ]; then
              npm ci
            fi
            exec npm run tauri:dev
          '';
        };
        # The bundled codex binary that room-server spawns at runtime,
        # exposed as a flake output for diagnostic use (`nix run .#codex
        # -- doctor`) and so the version pinning is visible from
        # `nix flake show`.
        codex = pkgs.codex;
      });

      nixosModules.room = ./modules/services/room;
      nixosModules.symphony = ./modules/services/symphony;

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = [
            pkgs.rust-bin.nightly."2026-05-04".default
            pkgs.elixir_1_19
            pkgs.erlang_28
            pkgs.gh
            pkgs.git
            pkgs.openssh
            pkgs.codex
          ];
        };
      });
    };
}
