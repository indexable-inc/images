{ix}:
# The consumable artifact is the compiled `:gmail_ex` OTP app (generated
# Elixir host modules + the NIF under priv/native), laid out so a BEAM can
# `Code.append_path`/`Application.load` it at runtime -- ix-mcp-ex does
# exactly that via `IX_MCP_GMAIL_EX` (mirrors packages/tui/ex).
# `passthru.tests.run` compiles the mix package warnings-as-errors and
# drives the signed-out auth boundary through the generated bindings with
# ExUnit (offline on purpose: no credentials in the sandbox), gated in CI
# as `checks.<system>.google-gmail-ex-run`.
let
  inherit (ix) pkgs;

  built = ix.unibind.build {
    crate = "google-gmail-ex";
    targets.ex = {
      mixSource = ./mix;
    };
  };

  # 1.18/27, not the 1.19/28 the other unibind suites use: ix-mcp-ex loads
  # these .beam files at runtime and its release runs on 1.18/OTP 27, and
  # the BEAM does not load bytecode from a newer OTP than its own.
  elixir = ix.languages.elixir.toolchain pkgs {version = "1.18";};
  erlang = ix.languages.erlang.toolchain pkgs {version = "27";};

  version = "0.1.0"; # keep in sync with mix/mix.exs

  sharedEnv = {
    HEX_OFFLINE = "1";
    LANG = "C.UTF-8";
    LC_CTYPE = "C.UTF-8";
  };

  app = pkgs.stdenv.mkDerivation {
    pname = "google-gmail-ex";
    inherit version;
    src = built.ex.mixPackage;
    strictDeps = true;

    nativeBuildInputs = [erlang elixir];
    env = sharedEnv // {MIX_ENV = "prod";};

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

    # `-L` resolves mix's priv symlink so the app dir is self-contained;
    # the layout ($out/lib/gmail_ex/{ebin,priv}) is what `:code.priv_dir`
    # expects once $out/lib/gmail_ex/ebin is on the code path.
    installPhase = ''
      # shell
      runHook preInstall
      mkdir -p "$out/lib"
      cp -RL _build/prod/lib/gmail_ex "$out/lib/gmail_ex"
      runHook postInstall
    '';

    meta.description = "Compiled gmail_ex OTP app: unibind-generated Elixir bindings over the google-gmail client";
  };

  run = pkgs.stdenv.mkDerivation {
    pname = "google-gmail-ex-run";
    inherit version;
    src = built.ex.mixPackage;
    strictDeps = true;

    # The ExUnit suite has zero hex deps, so the sandbox never fetches;
    # HEX_OFFLINE guards regressions.
    nativeBuildInputs = [erlang elixir];
    env = sharedEnv // {MIX_ENV = "test";};

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
      mix test --trace
      runHook postCheck
    '';

    installPhase = ''
      # shell
      runHook preInstall
      mkdir -p "$out"
      runHook postInstall
    '';

    meta.description = "gmail Elixir binding smoke suite: the signed-out auth boundary driven through the generated bindings (ExUnit)";
  };
in
  app.overrideAttrs (old: {
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
