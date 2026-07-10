{home-manager}: let
  render = home-manager.lib.hm.generators.toKDL {
    escapeBackslashes = true;
  };
in {
  inherit render;
  generate = pkgs: name: value: pkgs.writeText name (render value);
}
