{ix}:
# Prove the generated Elixir surface works: take the mix package assembled
# by `unibind.build` (generated lib/, priv/native NIF, the hand-written
# ExUnit suite in ./mix), compile it warnings-as-errors, and run `mix test`
# offline. Exposed as `passthru.tests.run`, gated in CI as
# `checks.<system>.plumb-ex-run`. Mirrors packages/unibind/conformance-ex.
let
  inherit (ix) pkgs;

  built = ix.unibind.build {
    crate = "plumb-ex";
    targets.ex = {
      mixSource = ./mix;
    };
  };

  elixir = ix.languages.elixir.toolchain pkgs {version = "1.19";};
  erlang = ix.languages.erlang.toolchain pkgs {version = "28";};

  run = pkgs.stdenv.mkDerivation {
    pname = "plumb-ex-run";
    version = "0.1.0";
    src = built.ex.mixPackage;
    strictDeps = true;

    # The ExUnit suite has zero hex deps, so the sandbox never fetches;
    # HEX_OFFLINE guards regressions.
    nativeBuildInputs = [erlang elixir];
    env = {
      MIX_ENV = "test";
      HEX_OFFLINE = "1";
      LANG = "C.UTF-8";
      LC_CTYPE = "C.UTF-8";
    };

    # Mix 1.18+ opens a loopback socket unconditionally (Mix.Sync.PubSub)
    # and the darwin sandbox denies it (:eperm) without this.
    __darwinAllowLocalNetworking = true;

    postUnpack = ''
      export HOME="$TEMPDIR"
      export MIX_HOME="$TEMPDIR/mix"
      export HEX_HOME="$TEMPDIR/hex"
    '';

    buildPhase = ''
      # shell
      runHook preBuild
      mix compile --warnings-as-errors
      runHook postBuild
    '';

    doCheck = true;
    checkPhase = ''
      # shell
      runHook preCheck
      mix format --check-formatted
      # --trace prints every test name and runs sequentially.
      mix test --trace
      runHook postCheck
    '';

    installPhase = ''
      # shell
      runHook preInstall
      mkdir -p "$out"
      runHook postInstall
    '';

    meta.description = "plumb Elixir binding smoke suite over the generated bindings (ExUnit)";
  };
in
  run.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests =
          (old.passthru.tests or {})
          // {
            inherit run;
          };
      };
  })
