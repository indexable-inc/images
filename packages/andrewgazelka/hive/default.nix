# Hive: a tiny fully-connected mesh of agent actors (GenServer + Registry +
# DynamicSupervisor), moved out of a standalone checkout to exercise the repo's
# Elixir type-discipline + Credo gates.
#
# The required quality lane (compile --warnings-as-errors, which runs Elixir
# 1.18's set-theoretic type checker, plus format, `mix credo --strict` against
# the shared lib/elixir/credo.exs, and test) is built through the shared
# ix.buildElixirCheck lane and exposed as `passthru.tests.elixir`, wired into
# `checks` through lib/per-system.nix, exactly like symphony.
#
# credo is the sole hex dep and is test-only (see mix.exs), so the `hive`
# launcher still runs `mix` offline in :dev with no deps; only the sandboxed
# check fetches deps, via the fixed-output `mixFodDeps` below.
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
      ./elixir/mix.lock
      ./elixir/.formatter.exs
    ];
  };

  # Test-env mix deps (credo + its deps) as a fixed-output derivation so the
  # sandboxed check runs offline. Refresh the hash whenever elixir/mix.lock changes.
  mixFodDeps = pkgs.beamPackages.fetchMixDeps {
    pname = "hive-elixir-deps";
    version = "0.1.0"; # keep in sync with elixir/mix.exs
    src = lib.fileset.toSource {
      root = ./elixir;
      fileset = lib.fileset.unions [
        ./elixir/mix.exs
        ./elixir/mix.lock
      ];
    };
    inherit elixir;
    mixEnv = "test";
    hash = "sha256-EXaJddUakJETdzPNFWgJRgBWG4VcrP/Z5tOCuE+BXdo=";
  };

  elixirCheck = ix.buildElixirCheck pkgs {
    pname = "hive-elixir-check";
    version = "0.1.0";
    inherit src elixir erlang;
    mixDeps = mixFodDeps;
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
    # nu
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
