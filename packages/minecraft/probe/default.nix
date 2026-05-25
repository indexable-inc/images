{
  ix,
  pkgs ? ix.pkgs,
}:

ix.buildUvApplication pkgs {
  pname = "mc-probe";
  version = "0.1.0";
  srcRoot = ./.;
}
