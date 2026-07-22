# Fixture home-manager configuration for tests/provenance.nix. The walker
# assertions match definition sites against THIS file's name and line
# numbers, so it stays a separate file: inline modules would blur the
# user-site vs wiring-hop distinction the test exercises.
{pkgs, ...}: {
  home = {
    username = "test";
    homeDirectory = "/home/test";
    stateVersion = "25.05";
    file."provenance-test.txt".text = "provenance eval fixture";

    # Package provenance (#3942): a nixpkgs package's meta.position points
    # into nixpkgs, so it must degrade to this file with no line; the
    # inline derivation records meta.position off its literal `version`
    # attr (the same unsafeGetAttrPos machinery mkDerivation uses), so it
    # must carry this file's line. Raw `derivation` instead of
    # `pkgs.stdenv.mkDerivation` keeps this eval-only fixture (never built)
    # out of the package-function lint scope.
    packages = [
      pkgs.hello
      (let
        attrs = {
          pname = "provenance-inline";
          version = "1";
        };
        pos = builtins.unsafeGetAttrPos "version" attrs;
      in
        derivation {
          name = "${attrs.pname}-${attrs.version}";
          system = "x86_64-linux";
          builder = "/bin/false";
        }
        // {
          inherit (attrs) pname version;
          meta.position = "${pos.file}:${toString pos.line}";
        })
    ];
  };

  # Deployed through the xdg -> home.file wiring hop.
  xdg.configFile."provenance-test/config.toml".text = "x = 1";

  # Deployed through a settings-rendering program module: the manifest entry
  # for htoprc must chain back to this settings definition site.
  programs.htop = {
    enable = true;
    settings.color_scheme = 6;
  };
}
