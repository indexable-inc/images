# Symphony: the Elixir control plane for `.sym` agent workflows, absorbed
# from the dedicated indexable-inc/symphony repo at
# c9e709208c3ae161e24f625b9f3808a288c859ed. The launcher stages this source
# tree into the state dir at start and runs it with `mix run --no-halt`
# (see bin/run-nix), so the package is a closure around the source rather
# than a compiled BEAM release; production keeps deploying the same way it
# did from the standalone flake's `packages.default`.
#
# The room stack symphony drives over HTTP (room-server and the room UI)
# lives in the ix monorepo. TODO: re-add the `room-server` binary once the
# ix<->index flake cycle is resolved. Only the runtime moved here.
{
  lib,
  pkgs,
  ix,
  writeNushellApplication,
}:
let
  # mise.toml pins Elixir 1.19 on OTP 28; the launcher and the check build
  # against the same pairing so a deploy never runs code the gate did not.
  elixir = ix.languages.elixir.toolchain pkgs { version = "1.19"; };
  erlang = ix.languages.erlang.toolchain pkgs { version = "28"; };

  # The tree bin/run-nix stages at service start: the mix project, the
  # bundled example pack, and the engine wire fixtures. contracts/ sits
  # beside elixir/ because the contract tests resolve it relatively
  # (test/symphony_elixir/engine/contract_fixtures_test.exs).
  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./bin
      ./contracts
      ./elixir
      ./workflows
    ];
  };

  # SRI pins for the two fixed-output fetches below live in the sibling
  # pins.json (repo policy: no inline hash literals). `mix-deps` has no URL
  # (the FOD's content is derived from mix.lock, so refresh it by building
  # after a lock change and copying the `got:` hash); `lazy-html-nif` pins the
  # upstream release tarball by URL.
  pins = ix.pins.loadPins ./pins.json;

  # Test-env mix deps as a fixed-output derivation so the sandboxed check
  # runs offline. Refresh the hash whenever elixir/mix.lock changes.
  mixFodDeps = pkgs.beamPackages.fetchMixDeps {
    pname = "symphony-elixir-deps";
    version = "0.2.0"; # keep in sync with elixir/mix.exs
    src = lib.fileset.toSource {
      root = ./elixir;
      fileset = lib.fileset.unions [
        ./elixir/mix.exs
        ./elixir/mix.lock
      ];
    };
    inherit elixir;
    mixEnv = "test";
    inherit (pins."mix-deps") hash;
  };

  # mix.lock pins lazy_html (a C++ NIF over lexbor) as a test-only dep for
  # LiveView's HTML assertions. Compiling it either downloads this exact
  # precompiled NIF over mix's httpc or git-clones lexbor, and the sandbox
  # allows neither, so the check below seeds elixir_make's artifact cache
  # with the upstream release tarball; elixir_make still verifies it against
  # the checksum.exs pinned inside the dep before unpacking. Refresh the
  # url/hash in pins.json when a mix.lock bump moves lazy_html.
  lazyHtmlNif = pkgs.fetchurl { inherit (pins."lazy-html-nif") url hash; };

  # The required quality lane the standalone repo ran per PR (make ci:
  # compile --warnings-as-errors, format --check-formatted, credo, test),
  # now a sandboxed derivation wired into `checks` through per-system.nix.
  # The advisory lane (dialyzer, sobelow, mix_audit, coveralls) stays a
  # local `mix quality` run; see docs/quality.md.
  # Built through the shared lane (lib/build/elixir-check.nix) so the policy and
  # the credo config match every other Elixir package. `mix credo --strict` now
  # runs against the repo-wide strict config (lib/elixir/credo.exs).
  elixirCheck = ix.buildElixirCheck pkgs {
    pname = "symphony-elixir-check";
    version = "0.2.0";
    inherit src elixir erlang;
    sourceRoot = "source/elixir";
    mixDeps = mixFodDeps;
    extraEnv = {
      # rebar-built deps (telemetry) make mix install rebar over the network
      # unless the binaries are pinned, same as fetchMixDeps/mixRelease do.
      MIX_REBAR = "${pkgs.beamPackages.rebar}/bin/rebar";
      MIX_REBAR3 = "${pkgs.beamPackages.rebar3}/bin/rebar3";
      # The precompiled lazy_html .so is a generic linux-gnu build, so the
      # BEAM needs libstdc++ findable at dlopen time when the test suite
      # loads the NIF.
      LD_LIBRARY_PATH = lib.makeLibraryPath [ pkgs.stdenv.cc.cc.lib ];
    };
    # The elixir_make cache seed short-circuits the lazy_html NIF download (see
    # lazyHtmlNif above; mix/tasks/compile.elixir_make.ex reuses an existing
    # archive instead of fetching).
    setupHook = ''
      export ELIXIR_MAKE_CACHE_DIR="$TEMPDIR/elixir-make-cache"
      mkdir -p "$ELIXIR_MAKE_CACHE_DIR"
      cp "${lazyHtmlNif}" "$ELIXIR_MAKE_CACHE_DIR/${lazyHtmlNif.name}"
    '';
  };
in
(writeNushellApplication {
  name = "symphony";
  meta = {
    description = "Elixir runtime for .sym agent workflows (control plane, LiveView dashboard, triggers)";
    license = lib.licenses.asl20;
  };
  # codex is intentionally absent: bin/run-nix requires an authenticated
  # codex on the operator's PATH and refuses to start otherwise, so the
  # binary and its credentials stay host-owned.
  runtimeInputs = [
    pkgs.bash
    pkgs.cacert
    pkgs.coreutils
    elixir
    erlang
    pkgs.gh
    pkgs.git
    pkgs.openssh
  ];
  text = ''
    # nu
    def --wrapped main [...args] {
      exec ${src}/bin/run-nix ...$args
    }
  '';
}).overrideAttrs
  (old: {
    passthru = (old.passthru or { }) // {
      tests.elixir = elixirCheck;
    };
  })
