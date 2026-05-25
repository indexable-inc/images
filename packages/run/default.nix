{
  ix,
  lib,
  pkgs,
}:

let
  scriptreplay =
    if pkgs.stdenv.hostPlatform.isLinux then
      lib.getExe' pkgs.util-linux "scriptreplay"
    else
      "scriptreplay";
  unwrapped = ix.writePythonApplication pkgs {
    name = "run-unwrapped";
    src = ./run.py;
    meta.description = "Record a command's terminal session, timing, and queryable output events";
  };
  package =
    pkgs.runCommand "run"
      {
        nativeBuildInputs = [ pkgs.makeWrapper ];
        strictDeps = true;
        meta = {
          mainProgram = "run";
          description = "Run a command with terminal replay files and structured JSONL output";
        };
      }
      ''
        mkdir -p $out/bin
        makeWrapper ${lib.getExe unwrapped} $out/bin/run \
          --set IX_RUN_SCRIPTREPLAY ${lib.escapeShellArg scriptreplay}
      '';
  recordsSession =
    pkgs.runCommand "run-records-session"
      {
        nativeBuildInputs = [ package ];
        strictDeps = true;
      }
      ''
        export HOME=$TMPDIR/home
        export IX_RUN_DIR=$TMPDIR/runs
        export IX_RUN_HEAD_LINES=2
        export IX_RUN_TAIL_LINES=2
        mkdir -p "$HOME"

        run ${lib.getExe pkgs.bash} -c 'for n in 1 2 3 4 5 6; do printf "line-%s\n" "$n"; done' >stdout 2>stderr

        session=$(readlink "$IX_RUN_DIR/latest")
        test -d "$session"
        test -s "$session/output.log"
        test -s "$session/typescript"
        test -s "$session/timing.log"
        test -s "$session/events.jsonl"
        test -s "$session/lines.jsonl"
        test -s "$session/session.cast"
        test -x "$session/replay"
        test -x "$session/live"

        grep -q '"status": "exited"' "$session/summary.json"
        grep -q '"exit_code": 0' "$session/summary.json"
        grep -q '"line_no":6' "$session/lines.jsonl"
        grep -q 'line-4' "$session/output.log"
        grep -q 'line-1' stdout
        grep -q 'line-2' stdout
        grep -q 'line-5' stdout
        grep -q 'line-6' stdout
        if grep -q 'line-3' stdout; then
          echo "middle output leaked into summarized stdout" >&2
          exit 1
        fi
        grep -q 'Full live output\|live stream' stderr

        mkdir -p "$out"
      '';
in
package.overrideAttrs (old: {
  passthru = (old.passthru or { }) // {
    tests = {
      inherit recordsSession;
    };
  };
})
