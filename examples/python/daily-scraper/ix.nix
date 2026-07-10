{index}:
index.lib.mkFleet {
  nodes.scraper = {
    modules = [
      ./service.nix
    ];
  };
}
