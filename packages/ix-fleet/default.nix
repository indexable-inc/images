{
  ix,
  pkgs ? ix.pkgs,
}:

ix.buildUvApplication pkgs {
  pname = "ix-fleet";
  version = "0.1.0";
  srcRoot = ./.;
}
