# beamvm: a persistent BEAM node as a user service, with the applications it
# hosts declared in Nix and hot-swapped on update instead of restarted.
#
# Two binaries:
#   beamvm-harness  the long-lived node; reads a JSON manifest of apps
#                   (see harness.ex for the reload semantics)
#   beamvm-ctl      pokes the harness's unix control socket (reload/status/
#                   ping); home-manager activation calls `reload` after it
#                   moves the manifest symlink
#
# The toolchain pin matches symphony (Elixir 1.19 / OTP 28) on purpose: every
# tenant's compiled beams must load into this harness's ERTS, and the
# skip-if-loaded rule in the harness assumes the release's bundled elixir and
# stdlib are the very ones the harness booted.
{
  lib,
  pkgs,
  ix,
}: let
  elixir = ix.languages.elixir.toolchain pkgs {version = "1.19";};

  harnessEbin =
    pkgs.runCommand "beamvm-harness-ebin" {
      nativeBuildInputs = [elixir];
      src = ./harness.ex;
    } ''
      mkdir -p "$out"
      elixirc -o "$out" "$src"
    '';

  harness = ix.writeBashApplication pkgs {
    name = "beamvm-harness";
    runtimeInputs = [elixir];
    text = ''
      : "''${BEAMVM_STATE_DIR:?beamvm-harness: BEAMVM_STATE_DIR must be set}"
      : "''${BEAMVM_MANIFEST:?beamvm-harness: BEAMVM_MANIFEST must be set}"
      exec elixir -pa ${harnessEbin} --no-halt -e 'BeamVM.Harness.main()'
    '';
  };

  ctl = ix.writeBashApplication pkgs {
    name = "beamvm-ctl";
    runtimeInputs = [pkgs.socat pkgs.jq];
    text = ''
      usage() {
        echo "usage: beamvm-ctl --socket <path> <ping|status|reload>" >&2
        exit 64
      }

      socket=""
      cmd=""
      while [ $# -gt 0 ]; do
        case $1 in
          --socket) socket=$2; shift 2 ;;
          ping | status | reload) cmd=$1; shift ;;
          *) usage ;;
        esac
      done
      [ -n "$socket" ] && [ -n "$cmd" ] || usage

      # Exit 2, distinct from failure: "not running" is a legitimate state
      # during activation (first install, or the unit is stopped). The caller
      # decides whether that is fine (a fresh start reads the current
      # manifest anyway) or fatal.
      if [ ! -S "$socket" ]; then
        echo "beamvm-ctl: no control socket at $socket (vm not running)" >&2
        exit 2
      fi

      reply=$(printf '%s\n' "$cmd" | socat -t 60 - "UNIX-CONNECT:$socket")
      printf '%s\n' "$reply"
      [ "$(jq -r '.ok' <<<"$reply")" = "true" ]
    '';
  };

  # Proof the hot path works: boot the harness on a v1 demo app whose
  # gen_server heartbeats its compiled-in version and pid to a file, flip the
  # manifest symlink to v2, `beamvm-ctl reload`, and require the heartbeat to
  # show the new version from the SAME server pid in the SAME OS process --
  # i.e. a code swap, not any flavor of restart.
  hotReloadTest =
    pkgs.runCommand "beamvm-hot-reload-test" {
      nativeBuildInputs = [
        elixir
        harness
        ctl
        pkgs.jq
      ];
      demoV1 = ./test/demo-v1.ex;
      demoV2 = ./test/demo-v2.ex;
      demoApp = ./test/demo.app;
    } ''
      mkdir -p v1/ebin v2/ebin state
      elixirc -o v1/ebin "$demoV1"
      elixirc -o v2/ebin "$demoV2"
      cp "$demoApp" v1/ebin/demo.app
      cp "$demoApp" v2/ebin/demo.app

      for v in v1 v2; do
        cat > "manifest-$v.json" <<EOF
      {"apps": {"demo": {"code_path_globs": ["$PWD/$v/ebin"], "start": true}}}
      EOF
      done

      ln -sf "$PWD/manifest-v1.json" manifest.current
      export BEAMVM_STATE_DIR="$PWD/state"
      export BEAMVM_MANIFEST="$PWD/manifest.current"
      export DEMO_OUT="$PWD/heartbeat"

      beamvm-harness > harness.log 2>&1 &
      harness_pid=$!

      sock="$BEAMVM_STATE_DIR/control.sock"
      for _ in $(seq 100); do
        if beamvm-ctl --socket "$sock" ping > /dev/null 2>&1; then break; fi
        sleep 0.1
      done
      beamvm-ctl --socket "$sock" ping > /dev/null || {
        echo "harness never became ready; its log follows" >&2
        cat harness.log >&2
        exit 1
      }

      # v1 heartbeats (100 ms period): capture the server pid.
      for _ in $(seq 50); do
        if grep -q "vsn=1" "$DEMO_OUT" 2>/dev/null; then break; fi
        sleep 0.1
      done
      grep -q "vsn=1" "$DEMO_OUT"
      pid_v1=$(grep -m1 "vsn=1" "$DEMO_OUT" | sed 's/.*pid=//')

      ln -sf "$PWD/manifest-v2.json" manifest.current
      beamvm-ctl --socket "$sock" reload | tee reload.json
      [ "$(jq -r '.ok' reload.json)" = "true" ]

      for _ in $(seq 50); do
        if grep -q "vsn=2" "$DEMO_OUT" 2>/dev/null; then break; fi
        sleep 0.1
      done
      grep -q "vsn=2" "$DEMO_OUT"
      pid_v2=$(grep -m1 "vsn=2" "$DEMO_OUT" | sed 's/.*pid=//')

      # The same gen_server pid heartbeating the new version is the whole
      # point: code moved, the process (and its state) did not.
      [ "$pid_v1" = "$pid_v2" ] || {
        echo "server pid changed across reload: $pid_v1 -> $pid_v2 (restart, not hot swap)" >&2
        cat harness.log >&2
        exit 1
      }

      kill "$harness_pid"
      cat harness.log
      touch "$out"
    '';
in
  (pkgs.symlinkJoin {
    name = "beamvm";
    paths = [
      harness
      ctl
    ];
    meta = {
      description = "Persistent BEAM VM user service: Nix-declared OTP apps, hot code reload on update";
      license = lib.licenses.asl20;
      mainProgram = "beamvm-ctl";
    };
  })
  .overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        inherit harness ctl;
        tests.hot-reload = hotReloadTest;
      };
  })
