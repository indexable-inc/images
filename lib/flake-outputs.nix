# Cross-system assembly of the flake's collected outputs (#3899): collecting
# each per-system attrset, the Linux-to-Darwin alias graft, the
# required-gate root union, and the security-root alias surface. Kept out of
# flake.nix so the top level reads as a manifest of inputs and output
# categories; `perSystem` is the per-dev-system map built by
# lib/per-system.nix.
{
  lib,
  perSystem,
}: let
  collect = key: lib.mapAttrs (_: out: out.${key}) perSystem;
  linuxDarwinAliases = perSystem.x86_64-linux.darwinPackageAliases or {};
  # Graft the Linux-to-Darwin cross aliases over a collected per-system set so
  # a Darwin namespace resolves an aliased attr to the cross-compiled
  # x86_64-linux derivation instead of a native rebuild. Applied to both
  # `packages` (the consumer surface) and `cachePushRoots` (what
  # cache-push.yml publishes): the Darwin cache lane realises the post-alias
  # set filtered to native aarch64-darwin drvs, so an alias-shadowed native
  # variant (e.g. dag-runner) is neither built nor published. Consumers can
  # never install it (#1890).
  withDarwinAliases = raw:
    raw
    // lib.genAttrs [
      "aarch64-darwin"
    ] (system: raw.${system} // (linuxDarwinAliases.${system} or {}));
  ciChecks = collect "ciChecks";
  cachePushRoots = withDarwinAliases (collect "cachePushRoots");
  rawSecurityRoots = collect "securityRoots";
  rawSecurityRootPaths = collect "securityRootPaths";
in {
  inherit collect ciChecks cachePushRoots;
  packages = withDarwinAliases (collect "packages");
  # One evaluator pool owns every required Linux build root. Prefix closure
  # roots so a package and a check may share their natural name without one
  # silently replacing the other. The explicit collision guard keeps a
  # future check named `closure-*` from weakening the gate.
  requiredGateRoots =
    lib.mapAttrs (
      system: systemChecks: let
        closureRoots =
          lib.mapAttrs' (
            name: root: lib.nameValuePair "closure-${name}" root
          )
          cachePushRoots.${system};
        collisions = builtins.attrNames (lib.intersectAttrs systemChecks closureRoots);
      in
        assert lib.assertMsg (collisions == [])
        "requiredGateRoots.${system}: prefixed cache roots collide with ciChecks: ${builtins.concatStringsSep ", " collisions}";
          systemChecks // closureRoots
    )
    ciChecks;
  securityRoots =
    rawSecurityRoots
    // {
      aarch64-darwin =
        rawSecurityRoots.aarch64-darwin
        // lib.mapAttrs (
          name: _:
            (rawSecurityRoots.aarch64-darwin.${name} or rawSecurityRoots.x86_64-linux.${name})
            // {
              attr = "packages.aarch64-darwin.${name}";
            }
        ) (linuxDarwinAliases.aarch64-darwin or {});
    };
  securityRootPaths =
    rawSecurityRootPaths
    // {
      aarch64-darwin = rawSecurityRootPaths.aarch64-darwin // (linuxDarwinAliases.aarch64-darwin or {});
    };
}
