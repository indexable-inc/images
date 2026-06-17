# Hive: a tiny fully-connected mesh of agent actors (GenServer + Registry +
# DynamicSupervisor), moved out of a standalone checkout to exercise the repo's
# Elixir type-discipline gate. It has no hex dependencies, so unlike
# packages/agent/symphony there is no fetchMixDeps step: `mix compile` and
# `mix test` run offline against the source alone.
#
# The required quality lane (compile --warnings-as-errors, which runs Elixir
# 1.18's set-theoretic type checker, plus format and test) is a sandboxed
# derivation exposed as `passthru.tests.elixir` and wired into `checks` through
# lib/per-system.nix, exactly like symphony.
{
  lib,
  pkgs,
  ix,
  writeNushellApplication,
}:
let
  # mix.exs declares `~> 1.18`; the launcher and the check build against the
  # same toolchain so a run never executes code the gate did not.
  elixir = ix.languages.elixir.toolchain pkgs { version = "1.18"; };
  erlang = ix.languages.erlang.toolchain pkgs { version = "27"; };

  src = lib.fileset.toSource {
    root = ./elixir;
    fileset = lib.fileset.unions [
      ./elixir/lib
      ./elixir/test
      ./elixir/mix.exs
      ./elixir/.formatter.exs
    ];
  };

  elixirCheck = pkgs.stdenv.mkDerivation {
    pname = "hive-elixir-check";
    version = "0.1.0";
    inherit src;

    nativeBuildInputs = [
      erlang
      elixir
      pkgs.git
    ];
    strictDeps = true;

    env = {
      MIX_ENV = "test";
      HEX_OFFLINE = "1";
      LANG = "C.UTF-8";
      LC_CTYPE = "C.UTF-8";
    };

    buildPhase = ''
      runHook preBuild
      export MIX_HOME="$TEMPDIR/mix"
      export HEX_HOME="$TEMPDIR/hex"
      # --warnings-as-errors makes the 1.18 type checker's findings (and any
      # other compile warning) fail the build, the actual "type check by default".
      mix compile --warnings-as-errors
      runHook postBuild
    '';

    doCheck = true;
    checkPhase = ''
      runHook preCheck
      mix format --check-formatted
      mix test
      runHook postCheck
    '';

    installPhase = ''
      runHook preInstall
      mkdir -p "$out"
      runHook postInstall
    '';
  };
in
(writeNushellApplication {
  name = "hive";
  meta = {
    description = "A tiny fully-connected mesh of Elixir agent actors; `hive` runs the demo";
    license = lib.licenses.asl20;
  };
  runtimeInputs = [
    pkgs.coreutils
    elixir
    erlang
  ];
  text = ''
    def --wrapped main [...args] {
      # mix compiles in place, so stage the read-only source into a writable
      # temp dir before running the demo.
      let work = (^mktemp -d | str trim)
      ^cp -rL --no-preserve=mode ${src}/. $"($work)/"
      cd $work
      with-env {
        MIX_ENV: "dev"
        HEX_OFFLINE: "1"
        MIX_HOME: $"($work)/.mix"
        HEX_HOME: $"($work)/.hex"
      } {
        ^mix run -e "Hive.demo()"
      }
    }
  '';
}).overrideAttrs
  (old: {
    passthru = (old.passthru or { }) // {
      tests = (old.passthru.tests or { }) // {
        elixir = elixirCheck;
      };
    };
  })
