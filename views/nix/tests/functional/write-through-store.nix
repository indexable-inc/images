with import ./config.nix;

mkDerivation {
  name = "write-through-store";
  builder = ./write-through-store.builder.sh;
}
