/**
Build the `health-checks` apps that bring every example fleet up in
parallel through `ix up`, verify the declared `ix.healthChecks`, and tear
the VMs down on completion.

Each example contributes a Nushell lifecycle script that sanity-checks
for the `ix` binary, force-deletes any leftover VM with the same node
name, invokes `ix up`, then force-deletes the VM again so the next
run starts from scratch and an unrelated VM is never left running
after a test.

`ix up --name-prefix health-check-` isolates the external VM and group
identities from production while each build still resolves the example's
unprefixed `nixosConfigurations.<node>` output.

Returns an attrset with two front-ends over the same lifecycle scripts:

- `dag`: the default `nix run .#health-checks` entry point. Runs the
  lifecycles in parallel via `dag-runner`, surfaces an inline indicatif
  spinner per task on a TTY (line output otherwise), captures stdout
  and stderr so failed nodes' logs are dumped at the end, and exits
  with the worst node exit code. Pass `--output json` to get an NDJSON
  event stream instead. This is the headless/CI path.
- `zellij`: the `nix run .#health-checks-zellij` entry point. Launches
  a zellij session with one tab per example fleet so each lifecycle's
  output stays in its own scrollback while it runs. No aggregated exit
  code (zellij exits 0 when the operator quits the session), so reserve
  this for interactive triage rather than pass/fail gating.
*/
{
  lib,
  pkgs,
  kdl,
  writeNushellApplication,
  dagRunner,
}: {
  exampleFleets,
  exampleSources,
  exampleNames ? lib.attrNames exampleFleets,
}: let
  jsonFormat = pkgs.formats.json {};

  ixTokenCheck = ''
    let ix_token = ($env.IX_TOKEN? | default "" | str trim)
    if $ix_token == "" {
      print -e "IX_TOKEN is not set; export it before running health-checks"
      exit 1
    }
  '';

  ixTokenPrompt = ''
    mut ix_token = ($env.IX_TOKEN? | default "" | str trim)
    if $ix_token == "" {
      $ix_token = (try { input --suppress-output "IX_TOKEN: " } catch { "" } | str trim)
      print ""
    }

    if $ix_token == "" {
      print -e "IX_TOKEN is required to run health-checks"
      exit 1
    }

    $env.IX_TOKEN = $ix_token
  '';

  mkLifecycle = name: fleet: let
    prefix = "health-check-";
    nodes = map (node: prefix + node) fleet.planValue.order;
    source = exampleSources.${name};
  in
    writeNushellApplication pkgs {
      name = "health-check-${name}";
      text = ''
        # nu
        def main [] {
          let home = ($env.HOME? | default "")
          if $home != "" {
            $env.PATH = [$"($home)/.local/bin"] ++ $env.PATH
          }

          if (which ix | is-empty) {
            print -e $"[${name}] ix binary not found in PATH"
            print -e "  PATH segments:"
            for p in $env.PATH {
              print -e $"    ($p)"
            }
            print -e "  install the ix CLI into ~/.local/bin (or another PATH directory) before running health-checks"
            exit 1
          }

          ${ixTokenCheck}

          let up_help = (^ix up --help | complete)
          if $up_help.exit_code != 0 or not ($up_help.stdout | str contains "--name-prefix") {
            print -e "health-checks requires ix#7059: ix up must support --name-prefix before any VM is changed"
            exit 1
          }

          let nodes = ${builtins.toJSON nodes}

          print $"[${name}] removing any pre-existing VMs: ($nodes | str join ', ')"
          for node_name in $nodes {
            do --ignore-errors { ^ix rm --force $node_name } | ignore
          }

          print $"[${name}] booting and running health checks"
          cd ${source}
          try {
            ^ix up --name-prefix ${prefix}
          } catch { }
          let exit_code = ($env.LAST_EXIT_CODE? | default 1)

          print $"[${name}] tearing down"
          for node_name in $nodes {
            do --ignore-errors { ^ix rm --force $node_name } | ignore
          }

          exit $exit_code
        }
      '';
    };

  lifecycles = lib.mapAttrs mkLifecycle exampleFleets;
  lifecyclePackages =
    lib.mapAttrs' (
      name: lifecycle: lib.nameValuePair "health-check-${name}" lifecycle
    )
    lifecycles;

  spec = {
    nodes =
      lib.mapAttrs (_name: lifecycle: {
        command = [(lib.getExe lifecycle)];
      })
      lifecycles;
  };

  specFile = jsonFormat.generate "health-checks-dag.json" spec;

  dag = writeNushellApplication pkgs {
    name = "health-checks";
    meta.description = "Boot every example fleet in parallel, run its health checks, and tear the VMs down";
    runtimeInputs = [dagRunner];
    text = ''
      # nu
      def --wrapped main [...args] {
        exec ${lib.getExe dagRunner} ...$args ${specFile}
      }
    '';
  };

  # One tab with a pane per lifecycle so the whole run is visible at once.
  # Panes stay open after their command exits so the post-mortem output is
  # reachable; quit the session with Ctrl+q.
  zellijLayout = kdl.generate pkgs "health-checks-layout.kdl" {
    layout.tab = {
      _props.name = "health-checks";
      _children =
        map (name: {
          pane._props = {
            inherit name;
            command = lib.getExe lifecycles.${name};
          };
        })
        exampleNames;
    };
  };

  zellij = writeNushellApplication pkgs {
    name = "health-checks-zellij";
    meta.description = "Boot every example fleet, run its health checks, and view each in a zellij pane";
    runtimeInputs = [pkgs.zellij];
    text = ''
      # nu
      def main [] {
        ${ixTokenPrompt}

        exec zellij --layout ${zellijLayout}
      }
    '';
  };
in {
  inherit
    dag
    lifecyclePackages
    zellij
    ;
}
