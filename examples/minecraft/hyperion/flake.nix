{
  description = "ix example: hyperion, one game server behind three proxies";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    index = {
      url = "github:indexable-inc/index";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # Deliberately not following nixpkgs. hyperion builds against its own
    # pinned rust-overlay and nixpkgs, and a follow here swaps the toolchain
    # under a nightly-only workspace.
    hyperion.url = "github:hyperion-mc/hyperion";
  };

  outputs = {
    nixpkgs,
    index,
    hyperion,
    ...
  }: let
    # `default.ix` is JavaScript-syntax Nix, converted during evaluation.
    fleet = index.lib.importIxWasm ./default.ix {inherit index hyperion;};

    # Every system this fleet's commands can be typed from. The guests are
    # always x86_64-linux; this is the set of machines that can evaluate them.
    hostSystems = [
      "aarch64-darwin"
      "aarch64-linux"
      "x86_64-darwin"
      "x86_64-linux"
    ];

    # Force every node's toplevel and record the derivation it resolved to,
    # WITHOUT building any of them. `unsafeDiscardStringContext` is what makes
    # that possible: the string still has to be computed, so the whole module
    # system for all four nodes is evaluated and any option type error, missing
    # attribute or port collision throws here -- but with the context stripped
    # the check no longer depends on those closures, so it costs seconds
    # instead of the ~8 minutes a real fleet build takes.
    #
    # This exists because nothing else in the tree evaluates this example.
    # `exampleFleetsFor` (index's `lib/discovery.nix`) skips it by design:
    # `default.ix` takes a `hyperion` argument the aggregator has no way to
    # supply, and skipping rather than failing is what lets an example depend
    # on a service repo at all. ENG-10986.
    #
    # DO NOT "FIX" THAT BY TEACHING THE AGGREGATOR TO SUPPLY `hyperion`. It is
    # the obvious move and it is the wrong one: hyperion deliberately does not
    # follow this flake's nixpkgs (see the input comment above), so handing it
    # to the aggregator pulls hyperion's own nixpkgs and rust-overlay into
    # index's ROOT lock. That is exactly the closure invalidation behind the
    # 2026-07-20 decision to disable index's scheduled flake bumps, and it
    # would drift index's CI ahead of the workstation and fleet pins. Keeping
    # the check here, on this directory's own lock, is what keeps hyperion out
    # of index entirely.
    #
    # What it caught the day it was written: hyperion#1078 changed
    # `services.hyperion-proxy.gameServer` from a `host:port` string to a
    # submodule, and `proxy.nix` still passed a string. The pin could not
    # advance at all, and neither repository had a gate that could see it.
    # ENG-11448.
    fleetEvalFor = system: let
      pkgs = nixpkgs.legacyPackages."${system}";
      lines =
        nixpkgs.lib.mapAttrsToList
        (name: cfg: "${name} ${builtins.unsafeDiscardStringContext cfg.config.system.build.toplevel.drvPath}")
        fleet.nixosConfigurations;
    in
      pkgs.runCommand "hyperion-fleet-eval" {
        __structuredAttrs = true;
        # Newline-joined so the output doubles as the diff-test artifact the
        # README describes: two runs of this check, diffed, say which nodes a
        # change actually moves.
        drvPaths = builtins.concatStringsSep "\n" lines;
        # Guard the guard, BY NAME rather than by count. An empty
        # `nixosConfigurations` would make the line above vacuously true and
        # this check would pass having evaluated nothing -- a green tick
        # meaning "found no nodes", which is indistinguishable from "every
        # node is fine". A bare count would catch that, but it would also
        # break the moment somebody legitimately changes `replicas` in
        # `default.ix`, which is a digit this fleet is meant to be able to
        # turn. So require the one node that must always exist, plus at least
        # one proxy, and stay silent about how many proxies there are.
        # Space-joined rather than a list: `__structuredAttrs` turns a Nix
        # list into a bash ARRAY, so `"$nodeNames"` would silently be only its
        # first element and the loop below would test one name while looking
        # like it tested all of them.
        nodeNames = builtins.concatStringsSep " " (builtins.attrNames fleet.nixosConfigurations);
      } ''
        for required in hyperion-game hyperion-proxy-0; do
          case " $nodeNames " in
            *" $required "*) ;;
            *)
              echo "hyperion-fleet-eval: $required is not among the evaluated nodes ($nodeNames)" >&2
              exit 1
              ;;
          esac
        done
        printf '%s\n' "$drvPaths" > "$out"
      '';
  in {
    inherit (fleet) nixosConfigurations;

    # `nix flake check` in this directory is the gate, and it is the same
    # command CI runs, so CI cannot drift from what a contributor can type.
    checks = nixpkgs.lib.genAttrs hostSystems (system: {
      fleet-eval = fleetEvalFor system;
    });
    # One build for the whole fleet. The nodes share 99.8% of their closure, so
    # building them together costs barely more than building one, and `ix apply`
    # then exports the finished system to each VM rather than asking every guest
    # to compile the same thing over again. The proxies are `replicas` of one
    # spec in `default.ix`, so each still needs naming here: raising the count
    # adds a `-system` attr and an apply target, not a node definition.
    #
    #   nix build .#hyperion-game-system .#hyperion-proxy-0-system .#hyperion-proxy-1-system .#hyperion-proxy-2-system
    #   ix apply .#hyperion-game .#hyperion-proxy-0 .#hyperion-proxy-1 .#hyperion-proxy-2
    #
    # Exposed under every system, not only `x86_64-linux`, because these are
    # x86_64-linux guest systems whatever machine you build them from: the
    # machine that types `nix build` contributes a builder, not an identity.
    # Without this the command above is a missing-attribute error on the Mac it
    # is most likely to be typed on.
    packages = nixpkgs.lib.genAttrs hostSystems (_: fleet.systemPackages);
  };
}
