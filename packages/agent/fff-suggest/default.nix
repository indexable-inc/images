{
  lib,
  ix,
  fff,
  stdenv,
  makeBinaryWrapper,
  runCommand,
}:
# fff-suggest is a repo-owned rust workspace crate (the binary rides
# `ix.rustWorkspace.units` like `claude-hooks`), wrapped so it carries the path
# to the `libfff_c` cdylib it `dlopen`s at runtime. Baking `IX_FFF_LIB` makes the
# binary self-contained under any PATH (the daemon needs no extra env), exactly
# how `claude-code/hooks.nix` bakes `IX_GIT` for the hook binary.
let
  libName = if stdenv.hostPlatform.isDarwin then "libfff_c.dylib" else "libfff_c.so";
  fffLib = "${fff}/lib/${libName}";

  raw = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "fff-suggest";
    meta = {
      description = "fff-backed @ file completer for Claude Code (resident daemon + per-keystroke client)";
      license = lib.licenses.mit;
      mainProgram = "fff-suggest";
    };
  };

  wrapped =
    runCommand "fff-suggest"
      {
        nativeBuildInputs = [ makeBinaryWrapper ];
        passthru = (raw.passthru or { }) // {
          tests = (raw.passthru.tests or { }) // {
            e2e = e2eTest;
          };
        };
        meta = raw.meta or { };
      }
      ''
        makeBinaryWrapper ${raw}/bin/fff-suggest $out/bin/fff-suggest \
          --set IX_FFF_LIB ${fffLib}
      '';

  # End-to-end: stand up the daemon over a temp tree and confirm the client
  # returns the fff-ranked file for a query. A tiny idle timeout reaps the
  # daemon, so the test needs no platform-specific killer (`procps`/`pkill` is
  # Linux-only in nixpkgs and would make this check unbuildable on Darwin, where
  # this stack is documented as supported).
  e2eTest = runCommand "fff-suggest-e2e" { } ''
    set -eu
    export HOME="$TMPDIR/home"
    export XDG_RUNTIME_DIR="$TMPDIR/run"
    export IX_FFF_SUGGEST_IDLE_MS=1500
    mkdir -p "$HOME" "$XDG_RUNTIME_DIR"

    work="$TMPDIR/work"
    mkdir -p "$work/src"
    : > "$work/src/alpha_module.rs"
    : > "$work/src/beta_module.rs"
    : > "$work/README.md"
    cd "$work"

    # First query cold-starts the daemon; retry briefly while it scans.
    hits=""
    for _ in $(seq 1 50); do
      hits="$(printf '{"query":"alpha"}' | ${wrapped}/bin/fff-suggest query || true)"
      case "$hits" in *alpha_module.rs*) break ;; esac
      sleep 0.1
    done

    # The daemon we cold-started reaps itself via IX_FFF_SUGGEST_IDLE_MS, so
    # there is nothing to kill and the build never waits on a lingering process.

    case "$hits" in
      *alpha_module.rs*)
        echo "ok: client returned fff-ranked match"
        printf '%s\n' "$hits"
        ;;
      *)
        echo "FAIL: expected alpha_module.rs in suggestions, got:" >&2
        printf '%s\n' "$hits" >&2
        exit 1
        ;;
    esac
    touch "$out"
  '';
in
wrapped
