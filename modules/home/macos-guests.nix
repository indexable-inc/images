# Declarative in-guest state for vmkit macOS guest VMs (index#3206, an
# increment of index#2682): declare a guest (ssh endpoint, auto-login user,
# display) plus the resources it must carry — launchd agents rendered from
# structured Nix attrs and pinned binaries — and get one idempotent
# `macos-guest-<name>` command that pushes only drift over ssh and bootstraps
# launchd (bootout + bootstrap in the gui domain) only for changed or
# unloaded agents. `macos-guest-<name> ssh` opens a shell, while `status`
# reports drift read-only.
#
# The guest bundle itself stays a stateful pet until #2683's mkMacGuest lands:
# VM creation, Apple ID sign-in, and the GUI-only TCC grants are manual
# bootstrap (see ./macos-guests/tcc-bootstrap.md). Consumers hold only data:
# the guest spec, plist attrs, and pinned binary packages (hashes live in the
# owning package's pins.json, never inline — see e.g. packages/bbctl).
{
  indexPackages,
  ix,
}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.macosGuests;
  homeDirectory = config.home.homeDirectory;
  indexPkgs = indexPackages pkgs.stdenv.hostPlatform.system;
  inherit (indexPkgs) vmkit;
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
        default = "${homeDirectory}/.local/share/vmkit/guests/${name}";
        description = "Host-side vmkit bundle directory for this guest.";
      };

      lifecycle = {
        enable = lib.mkOption {
          type = lib.types.bool;
          default = true;
          description = "Whether launchd keeps this guest running on the host.";
        };
        macAddress = lib.mkOption {
          type = lib.types.strMatching "[0-9a-f][26ae](:[0-9a-f]{2}){5}";
          description = "Stable locally administered unicast MAC address for the guest.";
        };
        logPath = lib.mkOption {
          type = lib.types.str;
          default = "${homeDirectory}/Library/Logs/macos-guest-${name}.log";
          description = "Host path receiving vmkit stdout and stderr.";
        };
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

      brews = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [];
        description = ''
          Homebrew formulae the guest must have, installed by the apply
          command when missing. Homebrew rather than nix because nix cannot
          install on a macOS guest at all: its installer creates a volume and
          adds a line to /etc/fstab, and writing that file is refused there
          even as root. Homebrew upgrades on its own schedule, so a formula
          here pins that the package exists, never which version.
        '';
      };

      beamNode = {
        enable = lib.mkEnableOption ''
          a BEAM node on the guest that the host kernel calls into over
          distributed Erlang. Persistent rather than a command per action,
          because a node can also push guest events back to the host
        '';
        name = lib.mkOption {
          type = lib.types.str;
          default = "ixagent@${config.ssh.host}";
          defaultText = lib.literalExpression ''"ixagent@''${ssh.host}"'';
          description = ''
            Node name. A long name, not a short one: `-sname` needs both ends
            to share a DNS suffix, which the vmnet bridge does not give them.
          '';
        };
        distPort = lib.mkOption {
          type = lib.types.port;
          default = 9100;
          description = ''
            Fixed distribution port. Unpinned, Erlang picks an ephemeral one
            per boot, which the host cannot reach across the bridge.
          '';
        };
        elixir = lib.mkOption {
          type = lib.types.str;
          default = "/opt/homebrew/bin/elixir";
          description = "Guest path to the elixir that runs the node.";
        };
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

    config = lib.mkIf config.beamNode.enable {
      brews = ["elixir"];
      binaries.ix-agent-node.source = beamNodeScript config;
      launchAgents."dev.ix.agent-node".config = {
        ProgramArguments = ["${guestHome config}/.local/bin/ix-agent-node"];
        EnvironmentVariables = {
          HOME = guestHome config;
          # launchd agents get the bare system PATH, and elixir execs `erl`
          # off PATH: without this the agent dies with "exec: erl: not found".
          PATH = "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin";
        };
        RunAtLoad = true;
        KeepAlive = true;
        ProcessType = "Background";
        StandardOutPath = "/tmp/ix-agent-node.log";
        StandardErrorPath = "/tmp/ix-agent-node.log";
      };
    };
  };

  # The node runs under the guest's own bash: a store shebang would name a
  # /nix path no macOS guest has.
  beamNodeScript = guest:
    pkgs.writeTextFile {
      name = "ix-agent-node";
      executable = true;
      text = ''
        #!/bin/bash
        export PATH="/opt/homebrew/bin:$PATH"
        exec ${guest.beamNode.elixir} \
          --erl "-kernel inet_dist_listen_min ${toString guest.beamNode.distPort} inet_dist_listen_max ${toString guest.beamNode.distPort}" \
          --name ${guest.beamNode.name} \
          --cookie "$(cat "$HOME/.erlang.cookie")" \
          -e 'Process.sleep(:infinity)'
      '';
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
      # The executable launchd runs, so the apply can restart an agent whose
      # plist is unchanged but whose program was just replaced. Without it a
      # pushed binary sits on disk while the old one keeps running.
      program = lib.head (agent.config.ProgramArguments or [""]);
    }) (lib.filterAttrs (_: agent: agent.enable) guest.launchAgents);

  # The per-guest data seam: everything the apply command (and, later, the
  # mkMacGuest runner machinery of index#2683) needs to know, as plain JSON.
  guestManifest = name: guest:
    jsonFormat.generate "macos-guest-${name}.json" {
      inherit name;
      inherit (guest) ssh autoLoginUser display vmkitGuestDir brews;
      home = guestHome guest;
      resources = guestResources guest;
    };

  applyFor = name: guest:
    ix.writeNushellApplication pkgs {
      name = "macos-guest-${name}";
      meta.description = "Manage declared state and open an ssh shell for the ${name} macOS guest";
      text = ''
        # nu
        const manifest_file = "${guestManifest name guest}"

        # Platform ssh only: macOS's Local Network privacy gate returns
        # EHOSTUNREACH ("No route to host") to the vmnet guest subnet for
        # non-platform binaries when the responsible process lacks the Local
        # Network grant, while /usr/bin/ssh is exempt (hit live: nix openssh
        # failed, /usr/bin/ssh connected, same process, same target).
        def ssh-run [tgt: string, cmd: string] {
          ^/usr/bin/ssh -o BatchMode=yes $tgt $cmd
        }

        def ssh-target [spec: record] {
          $"($spec.ssh.user)@($spec.ssh.host)"
        }

        # null when the file is absent on the guest.
        def remote-hash [tgt: string, path: string] {
          let probe = do { ^/usr/bin/ssh -o BatchMode=yes $tgt $"shasum -a 256 '($path)'" } | complete
          if $probe.exit_code == 0 { $probe.stdout | split row ' ' | first } else { null }
        }

        # One row per resource: local vs guest content hash.
        def drift-table [] {
          let spec = open $manifest_file
          let tgt = ssh-target $spec
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
              program: ($r.program? | default null)
            }
          }
        }

        # Idempotent apply: push only drifted resources; bootout + bootstrap
        # only agents that changed, are not loaded, or whose executable this
        # run replaced. Fails loudly on any hop; the terminal check reads each
        # agent's live launchd state.
        # Formulae first: a pushed binary may name the interpreter one of them
        # provides. `brew list` once rather than `brew install` per formula,
        # because an install that is already satisfied still costs a network
        # round trip to Homebrew's API.
        def ensure-brews [tgt: string, brews: list<string>] {
          if ($brews | is-empty) { return }
          let have = ssh-run $tgt "/opt/homebrew/bin/brew list --formula -1" | lines | str trim
          for formula in $brews {
            if $formula in $have {
              print $"in sync: brew ($formula)"
            } else {
              ssh-run $tgt $"/opt/homebrew/bin/brew install ($formula)"
              print $"installed: brew ($formula)"
            }
          }
        }

        def main [] {
          let spec = open $manifest_file
          let tgt = ssh-target $spec
          let uid = ssh-run $tgt "id -u" | str trim
          ensure-brews $tgt $spec.brews
          let rows = drift-table
          # An agent whose plist is unchanged still runs the old executable
          # after its binary is replaced, so a pushed binary restarts every
          # agent that names it.
          let replaced = $rows | where {|r| $r.kind == "binary" and (not $r.in_sync) } | get remote_path
          for row in $rows {
            if $row.in_sync {
              print $"in sync: ($row.target)"
            } else {
              # Stage-then-rename so a KeepAlive respawn never execs a
              # half-copied file; a running process keeps its old inode.
              let staged = $"($row.remote_path).staged"
              let mode = if $row.kind == "binary" { "755" } else { "644" }
              ssh-run $tgt $"mkdir -p '($row.remote_path | path dirname)'"
              ^/usr/bin/scp -q $row.source $"($tgt):($staged)"
              ssh-run $tgt $"chmod ($mode) '($staged)' && mv -f '($staged)' '($row.remote_path)'"
              print $"pushed: ($row.target)"
            }
            if $row.kind == "launch-agent" {
              let service = $"gui/($uid)/($row.label)"
              # `launchctl bootout` of an absent service is an error, not
              # idempotence, so probe with `print` instead of swallowing it.
              let loaded = (do { ^/usr/bin/ssh -o BatchMode=yes $tgt $"launchctl print ($service)" } | complete | get exit_code) == 0
              if (not $row.in_sync) or (not $loaded) or ($row.program in $replaced) {
                if $loaded {
                  ssh-run $tgt $"launchctl bootout ($service)"
                  # bootout is asynchronous: an immediate bootstrap races the
                  # teardown and fails with "Bootstrap failed: 5: Input/output
                  # error" (hit live on this guest), so poll until launchd
                  # reports the service gone.
                  mut gone = false
                  for _ in 1..20 {
                    if (do { ^/usr/bin/ssh -o BatchMode=yes $tgt $"launchctl print ($service)" } | complete | get exit_code) != 0 {
                      $gone = true
                      break
                    }
                    sleep 250ms
                  }
                  if not $gone {
                    error make {msg: $"($service) still loaded 5s after bootout"}
                  }
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
          let spec = open $manifest_file
          let rows = drift-table
          print ($rows | select kind target in_sync | table)
          let have = if ($spec.brews | is-empty) { [] } else {
            ssh-run (ssh-target $spec) "/opt/homebrew/bin/brew list --formula -1" | lines | str trim
          }
          let missing = $spec.brews | where {|f| $f not-in $have }
          if not ($missing | is-empty) {
            print $"missing brews: ($missing | str join ', ')"
          }
          if (not ($rows | all {|r| $r.in_sync })) or (not ($missing | is-empty)) {
            exit 1
          }
        }

        # Open an interactive guest shell or run one command.
        def --wrapped "main ssh" [...command: string] {
          let spec = open $manifest_file
          exec /usr/bin/ssh -o BatchMode=yes (ssh-target $spec) ...$command
        }
      '';
    };

  lifecycleAgent = _: guest: {
    enable = guest.lifecycle.enable;
    config = {
      ProgramArguments = [
        (lib.getExe vmkit)
        "run-macos"
        "--bundle"
        guest.vmkitGuestDir
        "--mac-address"
        guest.lifecycle.macAddress
      ];
      KeepAlive = true;
      RunAtLoad = true;
      ProcessType = "Background";
      ThrottleInterval = 10;
      # Backstop only: must exceed vmkit's 10s shutdown-request grace plus 5s
      # force-stop grace, so the in-process escalation runs before launchd
      # SIGKILLs the VMM (index#3766).
      ExitTimeOut = 20;
      StandardOutPath = guest.lifecycle.logPath;
      StandardErrorPath = guest.lifecycle.logPath;
    };
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
      binaries) is declared here and managed over ssh by the generated
      `macos-guest-<name>` command. See modules/home/macos-guests/tcc-bootstrap.md
      for the manual TCC bootstrap a fresh guest still needs.
    '';
  };

  config = lib.mkIf (cfg != {}) {
    assertions = labelAssertions;
    home.packages = lib.mapAttrsToList applyFor cfg;
    launchd.agents = lib.mapAttrs lifecycleAgent cfg;
  };
}
