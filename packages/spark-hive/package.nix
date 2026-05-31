{
  id = "spark-hive";
  # The spark service module that consumes this is x86_64-linux only (the Gluten
  # Velox bundle has no other native build), so gate the flake output and
  # package-set attr to x86_64-linux to keep `nix flake check` from fetching the
  # ~400 MiB distribution on platforms nothing builds it for. Overlay stays
  # unconditional and lazy, mirroring spark-gluten and drgn.
  packageSet.systems = [ "x86_64-linux" ];
  flake.systems = [ "x86_64-linux" ];
  overlay = true;
}
