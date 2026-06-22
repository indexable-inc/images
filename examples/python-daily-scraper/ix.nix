{ index }:

index.lib.mkFleet {
  defaults = [ { ix.image.tag = "daily-scraper"; } ];

  nodes.scraper = {
    modules = [
      ./service.nix
    ];
  };
}
