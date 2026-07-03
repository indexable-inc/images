/**
Build the `health-checks` apps that bring every example fleet up in
parallel, verify the declared `ix.healthChecks` via the existing
`ix-fleet up` polling loop, and tear the VMs down on completion.

Each example contributes a Nushell lifecycle script that sanity-checks
for the `ix` binary, force-deletes any leftover VM with the same node
name, invokes `fleet.up`, then force-deletes the VM again so the next
run starts from scratch and an unrelated VM is never left running
after a test.

The fleets passed in carry a `health-check-` prefix applied by
`withNodePrefix`, so the names this script force-deletes cannot collide
with a production VM that happens to share the example's natural name
(`nginx`, `factions`, `file-server`, ...). The prefix is a plan-level
rename: the VMs boot the same NixOS closures (and base hostnames) as the
unprefixed example fleet, so the two surfaces share one evaluation.

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
  writeNushellApplication,
  dagRunner,
}: {
  exampleFleets,
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
    # Pin each node's OCI image as a build-time dep of the lifecycle script
    # so `nix run .#health-checks` realises every image before the runner
    # starts. Without this, `ix-fleet up` calls `nix-store --realise` on the
    # image .drv at runtime, which then triggers an x86_64-linux build chain
    # on whatever host launched the runner. Surfacing the realise step as a
    # normal Nix build fails fast at one well-known boundary instead of five
    # parallel runners independently rediscovering a broken remote builder.
    pinnedImages = lib.attrValues fleet.packages;
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

          let pinned_images = ${builtins.toJSON pinnedImages}
          let plan_data = (open ${fleet.plan})
          let nodes = $plan_data.order

          print $"[${name}] ($pinned_images | length) image\(s) pinned in store; removing any pre-existing VMs: ($nodes | str join ', ')"
          for node_name in $nodes {
            do --ignore-errors { ^ix rm --force $node_name } | ignore
          }

          print $"[${name}] booting and running health checks"
          # Stream ix-fleet so dag-runner can show the live per-node step.
          try {
            ^${lib.getExe fleet.up}
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
  zellijLayout = pkgs.writeText "health-checks-layout.kdl" ''
    layout {
      tab name="health-checks" {
    ${lib.concatStringsSep "\n" (
      map (
        name: let
          lifecycle = lifecycles.${name};
        in ''
          pane name="${name}" command="${lib.getExe lifecycle}"
        ''
      )
      exampleNames
    )}
      }
    }
  '';

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
