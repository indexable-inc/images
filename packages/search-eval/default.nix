{
  ix,
  lib,
  pkgs,
}:

let
  # The real `search` binary under test, from the shared cargo workspace graph,
  # and the `claude` CLI for the agentic tier. Both go on the wrapper's PATH so
  # the harness drives the same artifacts a developer uses.
  searchBin = ix.rustWorkspace.units.binaries.search;
  claudeBin = pkgs.claude-code;

  # Corpus and eval sets ship as plain files (not wheel data): `search` needs a
  # real directory to index. The wrapper points SEARCH_EVAL_DATA_DIR here.
  data = pkgs.runCommand "search-eval-data" { strictDeps = true; } ''
    mkdir -p "$out/corpus" "$out/datasets"
    cp -r ${./corpus}/. "$out/corpus/"
    cp -r ${./datasets}/. "$out/datasets/"
  '';

  unwrapped = ix.buildUvApplication pkgs {
    pname = "search-eval";
    version = "0.1.0";
    srcRoot = ./.;
    mainProgram = "search-eval";
    meta = {
      description = "Exa-style evaluation harness for the `search` engine";
      license = lib.licenses.mit;
      mainProgram = "search-eval";
    };
  };

  package =
    pkgs.runCommand "search-eval"
      {
        nativeBuildInputs = [ pkgs.makeWrapper ];
        strictDeps = true;
        meta = {
          description = "Exa-style evaluation harness for the `search` engine";
          license = lib.licenses.mit;
          mainProgram = "search-eval";
        };
      }
      ''
        mkdir -p $out/bin
        makeWrapper ${lib.getExe unwrapped} $out/bin/search-eval \
          --prefix PATH : ${
            lib.makeBinPath [
              searchBin
              claudeBin
            ]
          } \
          --set SEARCH_EVAL_DATA_DIR ${data}
      '';

  # Offline, deterministic: the ranking metrics are the CI-gating signal, so
  # they are unit-tested against the installed package with no network or key.
  metrics =
    pkgs.runCommand "search-eval-metrics"
      {
        strictDeps = true;
      }
      ''
        ${unwrapped}/venv/bin/python ${./tests/test_metrics.py}
        mkdir -p "$out"
      '';

  # No network and no API key: `--help` must exit 0 and name the program.
  printsHelp =
    pkgs.runCommand "search-eval-prints-help"
      {
        nativeBuildInputs = [ package ];
        strictDeps = true;
      }
      ''
        help=$(search-eval --help)
        case "$help" in
          *"search-eval"*) ;;
          *)
            echo "search-eval --help did not print usage" >&2
            printf '%s\n' "$help" >&2
            exit 1
            ;;
        esac
        mkdir -p "$out"
      '';
in
package.overrideAttrs (old: {
  passthru = (old.passthru or { }) // {
    tests = {
      inherit metrics printsHelp;
    };
  };
})
