{pkgs}: let
  # NASA Blue Marble equirectangular texture (public domain), pinned via
  # Wikimedia Commons; the renderer thresholds it into a land mask.
  texture = pkgs.fetchurl {
    url = "https://upload.wikimedia.org/wikipedia/commons/thumb/9/91/Land_shallow_topo_2048.jpg/3840px-Land_shallow_topo_2048.jpg";
    hash = "sha256-vAaf5YiPeJxQvHxc07q/tE0iD3bjJH4s7DKLvxLYUnQ=";
  };
  python = pkgs.python3.withPackages (ps: [
    ps.numpy
    ps.pillow
  ]);
in
  pkgs.runCommand "readme-globe" {nativeBuildInputs = [python];} ''
    mkdir -p $out
    python3 ${"${./render.py}"} ${"${texture}"} $out/globe.svg
  ''
