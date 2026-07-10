{ix}:
# The conformance crate ships nothing: the NIF library comes from the shared
# cargo-unit workspace graph, and the only artifact worth building is the
# proof that the generated Elixir surface behaves. This derivation *is* that
# proof: take the mix package assembled by `unibind.lib.build` (generated
# `lib/`, `priv/native/<soname>.so`, the hand-written ExUnit suite in
# ./mix), compile it warnings-as-errors, and run `mix test` offline. Exposed
# as `passthru.tests.run`, it joins the CI check set as
# `checks.<system>.unibind-conformance-ex-run`.
let
  inherit (ix) pkgs;

  built = ix.unibind.build {
    crate = "unibind-conformance-ex";
    targets.ex = {
      mixSource = ./mix;
    };
  };

  # The same toolchain pairing the repo's other Elixir gates build against
  # (packages/agent/symphony, packages/andrewgazelka/hive).
  elixir = ix.languages.elixir.toolchain pkgs {version = "1.19";};
  erlang = ix.languages.erlang.toolchain pkgs {version = "28";};

  run = pkgs.stdenv.mkDerivation {
    pname = "unibind-conformance-ex-run";
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
      # --trace prints every test name (the CI log is the conformance
      # evidence) and runs them sequentially, which the process-global
      # counters want anyway.
      mix test --trace
      runHook postCheck
    '';

    installPhase = ''
      # shell
      runHook preInstall
      mkdir -p "$out"
      runHook postInstall
    '';

    meta.description = "unibind phase-5 conformance suite over the generated Elixir bindings (ExUnit)";
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
