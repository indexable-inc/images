# Fixture home-manager configuration for tests/provenance.nix. The walker
# assertions match definition sites against THIS file's name and line
# numbers, so it stays a separate file: inline modules would blur the
# user-site vs wiring-hop distinction the test exercises.
{
  home = {
    username = "test";
    homeDirectory = "/home/test";
    stateVersion = "25.05";
    file."provenance-test.txt".text = "provenance eval fixture";
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
