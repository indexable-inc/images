# Declarative in-guest state for vmkit macOS guest VMs (index#3206, an
# increment of index#2682): declare a guest (ssh endpoint, auto-login user,
# display) plus the resources it must carry — launchd agents rendered from
# structured Nix attrs and pinned binaries — and get one idempotent
# `macos-guest-<name>` command that pushes only drift over ssh and bootstraps
# launchd (bootout + bootstrap in the gui domain) only for changed or
# unloaded agents. `macos-guest-<name> status` reports drift read-only.
#
# The guest bundle itself stays a stateful pet until #2683's mkMacGuest lands:
# VM creation, Apple ID sign-in, and the GUI-only TCC grants are manual
# bootstrap (see ./macos-guests/tcc-bootstrap.md). Consumers hold only data:
# the guest spec, plist attrs, and pinned binary packages (hashes live in the
# owning package's pins.json, never inline — see e.g. packages/bbctl).
{ix}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.macosGuests;
  jsonFormat = pkgs.formats.json {};

  # The one plist renderer: structured attrs in, XML out. User config never
  # hand-writes plist XML; the Label is owned by the attr name (asserted
  # below) so a plist can never disagree with the launchd service it names.
  renderAgent = label: agent:
    pkgs.writeText "${label}.plist" (
      lib.generators.toPlist {escape = true;} ({Label = label;} // agent.config)
    );

  guestModule = {
    name,
    config,
    ...
  }: {
    options = {
      ssh = {
        host = lib.mkOption {
          type = lib.types.str;
          description = "Guest address the apply command sshes to.";
        };
        user = lib.mkOption {
          type = lib.types.str;
          description = "Guest user the apply command sshes in as.";
        };
      };

      autoLoginUser = lib.mkOption {
        type = lib.types.str;
        default = config.ssh.user;
        defaultText = lib.literalExpression "ssh.user";
        description = ''
          Guest user owning the gui launchd domain the agents bootstrap into.
          Resource targets resolve relative to /Users/<autoLoginUser>.
        '';
      };

      display = lib.mkOption {
        type = lib.types.nullOr (lib.types.submodule {
          options = {
            width = lib.mkOption {
              type = lib.types.ints.positive;
              description = "Guest display width in pixels.";
            };
            height = lib.mkOption {
              type = lib.types.ints.positive;
              description = "Guest display height in pixels.";
            };
          };
        });
        default = null;
        description = ''
          Guest display geometry, recorded in the guest manifest for the
          vmkit runner seam (index#2683). Null leaves it to the bundle.
        '';
      };

      vmkitGuestDir = lib.mkOption {
        type = lib.types.str;
        default = "~/.local/share/vmkit/guests/${name}";
        description = "Host-side vmkit bundle directory for this guest.";
      };

      launchAgents = lib.mkOption {
        type = lib.types.attrsOf (lib.types.submodule {
          options = {
            enable = lib.mkOption {
              type = lib.types.bool;
              default = true;
              description = "Whether to push and bootstrap this agent.";
            };
            config = lib.mkOption {
              inherit (jsonFormat) type;
              description = ''
                launchd agent plist attrs (ProgramArguments, KeepAlive, ...),
                rendered with the module's single plist renderer. The Label
                key is injected from the attribute name; do not set it.
              '';
            };
          };
        });
        default = {};
        description = "launchd agents keyed by label, pushed to ~/Library/LaunchAgents on the guest.";
      };

      binaries = lib.mkOption {
        type = lib.types.attrsOf (lib.types.submodule ({name, ...}: {
          options = {
            source = lib.mkOption {
              type = lib.types.path;
              description = ''
                Executable to push, normally `lib.getExe <package>` of a
                package whose version + hash pin lives in its pins.json
                (repo policy: no inline hashes; `nix run .#update` re-pins).
              '';
            };
            target = lib.mkOption {
              type = lib.types.str;
              default = ".local/bin/${name}";
              description = "Install path on the guest, relative to the auto-login user's home.";
            };
          };
        }));
        default = {};
        description = "Pinned binaries pushed to the guest, keyed by command name.";
      };
    };
  };

  guestHome = guest: "/Users/${guest.autoLoginUser}";

  # Binaries first: an agent's first bootstrap may exec a binary pushed in
  # the same apply run.
  guestResources = guest:
    lib.mapAttrsToList (name: bin: {
      kind = "binary";
      inherit name;
      inherit (bin) source target;
    })
    guest.binaries
    ++ lib.mapAttrsToList (label: agent: {
      kind = "launch-agent";
      inherit label;
      source = renderAgent label agent;
      target = "Library/LaunchAgents/${label}.plist";
    }) (lib.filterAttrs (_: agent: agent.enable) guest.launchAgents);

  # The per-guest data seam: everything the apply command (and, later, the
  # mkMacGuest runner machinery of index#2683) needs to know, as plain JSON.
  guestManifest = name: guest:
    jsonFormat.generate "macos-guest-${name}.json" {
      inherit name;
      inherit (guest) ssh autoLoginUser display vmkitGuestDir;
      home = guestHome guest;
      resources = guestResources guest;
    };

  applyFor = name: guest:
    ix.writeNushellApplication pkgs {
      name = "macos-guest-${name}";
      runtimeInputs = [pkgs.openssh];
      meta.description = "Push declared state to the ${name} macOS guest and bootstrap launchd";
      text = ''
        # nu
        const manifest_file = "${guestManifest name guest}"

        def ssh-run [tgt: string, cmd: string] {
          ^ssh -o BatchMode=yes $tgt $cmd
        }

        # null when the file is absent on the guest.
        def remote-hash [tgt: string, path: string] {
          let probe = do { ^ssh -o BatchMode=yes $tgt $"shasum -a 256 '($path)'" } | complete
          if $probe.exit_code == 0 { $probe.stdout | split row ' ' | first } else { null }
        }

        # One row per resource: local vs guest content hash.
        def drift-table [] {
          let spec = open $manifest_file
          let tgt = $"($spec.ssh.user)@($spec.ssh.host)"
          $spec.resources | each {|r|
            let remote_path = $"($spec.home)/($r.target)"
            let local = open --raw $r.source | hash sha256
            let remote = remote-hash $tgt $remote_path
            {
              kind: $r.kind
              target: $r.target
              in_sync: ($remote == $local)
              remote_path: $remote_path
              source: $r.source
              label: ($r.label? | default null)
            }
          }
        }

        # Idempotent apply: push only drifted resources; bootout + bootstrap
        # only agents that changed or are not loaded. Fails loudly on any hop;
        # the terminal check reads each agent's live launchd state.
        def main [] {
          let spec = open $manifest_file
          let tgt = $"($spec.ssh.user)@($spec.ssh.host)"
          let uid = ssh-run $tgt "id -u" | str trim
          for row in (drift-table) {
            if $row.in_sync {
              print $"in sync: ($row.target)"
            } else {
              # Stage-then-rename so a KeepAlive respawn never execs a
              # half-copied file; a running process keeps its old inode.
              let staged = $"($row.remote_path).staged"
              let mode = if $row.kind == "binary" { "755" } else { "644" }
              ssh-run $tgt $"mkdir -p '($row.remote_path | path dirname)'"
              ^scp -q $row.source $"($tgt):($staged)"
              ssh-run $tgt $"chmod ($mode) '($staged)' && mv -f '($staged)' '($row.remote_path)'"
              print $"pushed: ($row.target)"
            }
            if $row.kind == "launch-agent" {
              let service = $"gui/($uid)/($row.label)"
              # `launchctl bootout` of an absent service is an error, not
              # idempotence, so probe with `print` instead of swallowing it.
              let loaded = (do { ^ssh -o BatchMode=yes $tgt $"launchctl print ($service)" } | complete | get exit_code) == 0
              if (not $row.in_sync) or (not $loaded) {
                if $loaded {
                  ssh-run $tgt $"launchctl bootout ($service)"
                }
                ssh-run $tgt $"launchctl bootstrap gui/($uid) '($row.remote_path)'"
              }
              let state = ssh-run $tgt $"launchctl print ($service)" | lines | where $it =~ 'state = ' | first | str trim
              print $"($row.label): ($state)"
            }
          }
        }

        # Read-only drift report; exits 1 when the guest diverges from the
        # declaration so it can gate automation.
        def "main status" [] {
          let rows = drift-table
          print ($rows | select kind target in_sync | table)
          if not ($rows | all {|r| $r.in_sync }) {
            exit 1
          }
        }
      '';
    };

  labelAssertions = lib.concatLists (lib.mapAttrsToList (
      name: guest:
        lib.mapAttrsToList (label: agent: {
          assertion = !(agent.config ? Label) || agent.config.Label == label;
          message = "macosGuests.${name}.launchAgents.\"${label}\": the Label key is owned by the attribute name; remove it from config.";
        })
        guest.launchAgents
    )
    cfg);
in {
  options.macosGuests = lib.mkOption {
    type = lib.types.attrsOf (lib.types.submodule guestModule);
    default = {};
    description = ''
      vmkit macOS guest VMs whose in-guest state (launchd agents, pinned
      binaries) is declared here and pushed over ssh by the generated
      `macos-guest-<name>` command. See modules/home/macos-guests/tcc-bootstrap.md
      for the manual TCC bootstrap a fresh guest still needs.
    '';
  };

  config = lib.mkIf (cfg != {}) {
    assertions = labelAssertions;
    home.packages = lib.mapAttrsToList applyFor cfg;
  };
}
