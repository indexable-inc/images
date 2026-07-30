{
  description = "ix example: templates-workers";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    index = {
      url = "github:indexable-inc/index";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    nixpkgs,
    index,
    ...
  }: let
    # `default.ix` is JavaScript-syntax Nix. `builtins.wasm` converts it during
    # evaluation, so evaluating this flake takes index's patched nix with
    # `wasm-builtin` in `extra-experimental-features` (`ix apply` and `ix eval`
    # pass the flag).
    config = index.lib.importIxWasm ./default.ix {inherit index;};

    # The seam this example exists for. `templates` and `instances` are exports
    # that the config's own `mkVm` calls know nothing about; this renders each
    # instance through its template and merges the result with the named VMs,
    # so `nixosConfigurations` carries `web`, `worker-1` and `worker-2` and
    # `ix apply` cannot tell which of them came from where. A config exporting
    # neither key comes back through here unchanged.
    rendered = index.lib.templates.renderConfig config;

    # Every system these commands can be typed from. The guests are always
    # x86_64-linux; this is the set of machines that can evaluate them.
    hostSystems = [
      "aarch64-darwin"
      "aarch64-linux"
      "x86_64-darwin"
      "x86_64-linux"
    ];
  in {
    inherit (rendered) nixosConfigurations;

    # `nix build .#worker-2-system` before `ix apply .#worker-2`, for an
    # instance and a named VM alike. Exposed under every system because these
    # are x86_64-linux guests whatever machine builds them: the machine that
    # types `nix build` contributes a builder, not an identity.
    packages = nixpkgs.lib.genAttrs hostSystems (_: rendered.systemPackages);
  };
}
