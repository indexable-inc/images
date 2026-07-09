# CAS layer: the manifest-native image built from the NixOS system closure.
#
# Unlike the OCI layer, the builder itself does not live in this repo: the ix
# repo owns manifest assembly (systemRoot, the baked nix store DB, init
# config) and threads its builder in as a fleet/image module, e.g.
#
#   mkFleet {
#     defaults = [{ix.build.casImageBuilder = ...;}];
#     ...
#   }
#
# `casImage` is deliberately lazy: nothing in this repo forces it during
# fleet-plan evaluation (the plan carries only the `.#<node>` installable
# string), so evaluating a fleet with no builder threaded stays valid.
# Forcing `casImage` without a builder aborts evaluation with the module
# system's "option `ix.build.casImageBuilder' ... used but not defined".
{
  config,
  lib,
  ...
}: {
  options.ix.build = {
    /**
    The ix-side manifest builder this repo codes against.

    Called as `casImageBuilder { toplevel = <NixOS system closure>; }`.
    Returns a derivation whose output directory contains `manifest.cas` (the
    serialized CAS manifest) and `locator.bin` (the chunk locator table), the
    pair `ix image push-manifest` consumes. The returned derivation must
    carry `passthru.toplevel = toplevel` so CI can gate on the system closure
    without building the manifest (the same property oci-layer.nix gives
    `ociImage`).
    */
    casImageBuilder = lib.mkOption {
      type = lib.types.functionTo lib.types.package;
      internal = true;
      description = "Builds the manifest-native CAS image for this node; provided by the ix repo (contract in the doc-comment above).";
    };
    casImage = lib.mkOption {
      type = lib.types.package;
      internal = true;
    };
  };

  config.ix.build.casImage = config.ix.build.casImageBuilder {
    inherit (config.system.build) toplevel;
  };
}
