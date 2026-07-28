{pkgs}: let
  # NASA Blue Marble equirectangular texture (public domain), pinned via
  # Wikimedia Commons; the renderer thresholds it into a land mask.
  # Pin the ORIGINAL upload, not a thumb: thumbnail bytes are re-encoded per
  # CDN datacenter and change over time, which broke this fixed-output hash
  # on 2026-07-19. The original file's bytes have been stable since 2017.
  texture = pkgs.fetchurl {
    url = "https://upload.wikimedia.org/wikipedia/commons/9/91/Land_shallow_topo_2048.jpg";
    hash = "sha256-Mb6MpsBHjwy8Tu6/h1cN/3TDUd5WCXLuDFCmMumEEZk=";
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
