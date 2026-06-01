{
  ix,
  lib,
  pkgs,
  ...
}:

let
  meta = {
    description = "HTTP server that ingests H.265 HLS screen streams, stores every segment per user/session, and serves them back for replay, live view, and indexing";
    license = lib.licenses.mit;
    mainProgram = "screencast-ingest";
  };

  server = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "screencast-ingest";
    inherit meta;
  };

  printsHelp =
    pkgs.runCommand "screencast-ingest-prints-help"
      {
        nativeBuildInputs = [ server ];
        strictDeps = true;
      }
      ''
        help=$(screencast-ingest --help)
        case "$help" in
          *"Usage: screencast-ingest"*) ;;
          *)
            echo "screencast-ingest --help did not print usage" >&2
            printf '%s\n' "$help" >&2
            exit 1
            ;;
        esac
        mkdir -p "$out"
      '';
in
server.overrideAttrs (old: {
  passthru = (old.passthru or { }) // {
    tests = (server.passthru.tests or { }) // {
      inherit printsHelp;
    };
  };
})
