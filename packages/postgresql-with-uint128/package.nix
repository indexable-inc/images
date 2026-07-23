{
  id = "postgresql-with-uint128";
  packageSet = true;
  flake.systems = [
    "x86_64-linux"
    "aarch64-linux"
  ];
  overlay.attrName = "postgresql_18_ix";
}
