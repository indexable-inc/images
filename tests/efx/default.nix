# The assembled terranix-port plan: the three ported stacks translated by
# `efx.fromTerranix`, plus the native replacement for the terraform
# `local_file`/`jsonencode` heartbeats export — an html.render effect that
# templates the minted heartbeat url into JSON, and a file.write that lands
# it, wired by ordinary efx references.
#
# ../efx-plan.nix asserts this plan equals the checked-in golden fixture
# (packages/efx/cli/tests/fixtures/terranix_port.plan.json), which the efx
# CLI's own tests consume through `efx plan --ir`. Regenerate the fixture
# after an intentional change with:
#
#   nix eval --json --impure --expr \
#     'let flake = builtins.getFlake (toString ./.); in
#      (import ./tests/efx {inherit (flake.inputs.nixpkgs) lib; ix = flake.lib;}).plan' \
#     | jq . > packages/efx/cli/tests/fixtures/terranix_port.plan.json
{
  lib,
  ix,
}: let
  inherit (ix) efx;
  inventory = import ./inventory.nix;
  stacks = {
    cloudflare = import ./cloudflare-stack.nix {inherit lib inventory;};
    ovh = import ./ovh-stack.nix {inherit lib inventory;};
    statusPage = import ./status-page-stack.nix;
  };

  heartbeatsRender = efx.effect {
    name = "heartbeats_json_render";
    kind = "html.render";
    inputs = {
      template = ''{"orchestrator-liveness": "{liveness}"}'';
      liveness = efx.ref "betteruptime_heartbeat.orchestrator_liveness" "url";
    };
  };
  heartbeatsFile = efx.effect {
    name = "heartbeats_json";
    kind = "file.write";
    inputs = {
      path = "generated/heartbeats.json";
      content = efx.ref "heartbeats_json_render" "html";
    };
  };

  effects =
    efx.fromTerranix {config = stacks.cloudflare;}
    ++ efx.fromTerranix {config = stacks.ovh;}
    ++ efx.fromTerranix {config = stacks.statusPage;}
    ++ [heartbeatsRender heartbeatsFile];
in {
  inherit effects inventory stacks;
  plan = efx.plan effects;
}
