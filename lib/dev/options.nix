/**
The `ix.dev.*` option surface (RFC 0007).

This is what lets a forked `ix.nix` read like an ordinary NixOS module: you
write `environment.systemPackages` and `programs.git.enable` at the top level
as usual, and reach for `ix.dev.*` only to describe the agents, the fleet
shape, and the shared identity volume. `mkDev` reads these options to plan the
fleet; the per-node build consumes `ix.dev.agents` to install the agent CLIs.

Declared in `lib/dev/` (not `modules/`) so it is only in scope where the dev
base imports it. It must not add `claude-code` to every image in the repo.
*/
{
  lib,
  pkgs,
  ...
}: let
  inherit
    (lib)
    mkOption
    mkEnableOption
    types
    literalExpression
    ;
in {
  options.ix.dev = {
    agents = {
      claude = mkOption {
        type = types.bool;
        default = true;
        description = "Install Claude Code (our sandbox-wrapped build) with the managed-settings policy.";
      };
      codex = mkOption {
        type = types.bool;
        default = true;
        description = "Install the Codex CLI.";
      };
    };

    profiles = {
      rust = {
        enable = mkEnableOption "the recommended Rust development toolchain";

        channel = mkOption {
          type = types.enum [
            "stable"
            "beta"
            "nightly"
          ];
          default = "nightly";
          description = "Rust release channel for the dev profile toolchain.";
        };

        version = mkOption {
          type = types.str;
          default = "latest";
          description = ''
            Rust toolchain version. Use `latest`, a stable semver, or a nightly
            date accepted by `ix.rustToolchainFor`.
          '';
        };

        components = mkOption {
          type = types.listOf types.str;
          default = [
            "cargo"
            "clippy"
            "llvm-tools-preview"
            "rust-analyzer"
            "rust-src"
            "rust-std"
            "rustc"
            "rustfmt"
          ];
          description = "rustup components included in the profile toolchain.";
        };

        targets = mkOption {
          type = types.listOf types.str;
          default = [];
          description = "Extra rustc targets installed with the profile toolchain.";
        };

        profile = mkOption {
          type = types.enum [
            "minimal"
            "default"
            "complete"
          ];
          default = "minimal";
          description = "rust-overlay profile baseline for the profile toolchain.";
        };

        packages = mkOption {
          type = types.listOf types.package;
          default = [
            pkgs.bacon
            pkgs.cargo-audit
            pkgs.cargo-deny
            pkgs.cargo-edit
            pkgs.cargo-expand
            pkgs.cargo-flamegraph
            pkgs.cargo-nextest
            pkgs.cargo-watch
            pkgs.clang
            pkgs.lldb
            pkgs.taplo
            pkgs.watchexec
          ];
          description = ''
            Extra Rust-adjacent tools installed by the profile. Extend or
            override this like any NixOS list option.
          '';
        };

        setEnvironment = mkOption {
          type = types.bool;
          default = true;
          description = "Set common Rust development environment variables.";
        };
      };
    };

    selfSource = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Materialize the dev source at `/ix` on every node so a VM can bring up
        more VMs from the same spec. On the shared volume when one exists, else
        a local writable copy.
      '';
    };

    fleet = mkOption {
      type = types.attrsOf (
        types.submodule {
          options = {
            replicas = mkOption {
              type = types.ints.positive;
              default = 1;
              description = "Number of interchangeable copies of this node.";
            };
            dependsOn = mkOption {
              type = types.listOf types.str;
              default = [];
              description = "Node names that must be up before this node.";
            };
            groups = mkOption {
              type = types.listOf types.str;
              default = [];
              description = "Extra private east-west groups this node joins.";
            };
            modules = mkOption {
              type = types.listOf types.raw;
              default = [];
              description = "Extra NixOS modules applied to this node only.";
            };
          };
        }
      );
      default = {
        dev = {};
      };
      example = literalExpression ''
        {
          agent.replicas = 3;
          builder.dependsOn = [ "agent" ];
        }
      '';
      description = ''
        Fleet topology: node name to spec, mirroring `mkFleet` nodes. The
        default is a single VM named `dev`; declaring nodes here replaces it.
      '';
    };

    shared = {
      enable = mkEnableOption "a shared SMB identity volume across the fleet";

      mountPoint = mkOption {
        type = types.str;
        default = "/shared";
        description = "Where the shared volume is mounted on each node.";
      };

      claude = mkOption {
        type = types.bool;
        default = true;
        description = ''
          Bind `~/.claude` onto the volume so the whole fleet shares one Claude
          login: the first `claude login` on any node logs in every node.
        '';
      };

      ix = mkOption {
        type = types.bool;
        default = false;
        description = ''
          Bind `~/.n` (the ix CLI credentials) onto the volume so any node can
          create more VMs. Sharper than `claude`: it hands out the ability to
          spawn VMs, so it is off by default.
        '';
      };

      excludeNodes = mkOption {
        type = types.listOf types.str;
        default = [];
        example = literalExpression ''[ "builder" ]'';
        description = "Nodes that opt out of the volume entirely (no mount, no shared identity).";
      };

      server = mkOption {
        type = types.str;
        default = "file-server";
        description = "Name of the dedicated node that runs `smbd` and holds the canonical files.";
      };

      group = mkOption {
        type = types.str;
        default = "ix-dev-shared";
        description = "Private east-west group the volume is reachable on, so it is never public.";
      };

      guestOk = mkOption {
        type = types.bool;
        default = true;
        description = ''
          Serve the share without authentication. The default keeps `ix up`
          working with no secrets plumbing (the share is still private to the
          group). Set false and add a Samba user for a production identity
          volume - see RFC 0007.
        '';
      };
    };
  };
}
