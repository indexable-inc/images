{
  ix,
  lib,
  pkgs ? ix.pkgs,
}: let
  catalog = ./catalog.json;
  inherit (lib.importJSON catalog) cards;
  # Hashes live in catalog.json, not inline here (repo pin policy).
  pdfFor = card:
    pkgs.fetchurl {
      name = "${card.slug}.pdf";
      inherit (card) url;
      hash = card.sha256;
    };
  pdfArgs = lib.concatMapStringsSep " " (card: "--pdf ${card.slug}=${pdfFor card}") cards;
  python = pkgs.python3.withPackages (ps: [ps.pymupdf4llm]);
  corpus =
    pkgs.runCommand "system-cards-corpus" {
      nativeBuildInputs = [python];
    } ''
      # shell
      python3 ${./convert.py} --catalog ${catalog} --out "$out" ${pdfArgs}
    '';
  application = ix.writePythonApplication pkgs {
    name = "system-cards-regen";
    src = ./regen.py;
    args = [
      "--corpus"
      corpus
    ];
    runtimeInputs = [pkgs.git];
    meta.description = "Sync the committed system-card markdown from the pinned PDF corpus build";
  };
in
  application.overrideAttrs (old: {
    passthru = (old.passthru or {}) // {inherit corpus;};
  })
