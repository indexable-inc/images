{
  lib,
  ix,
}: let
  # Read the package set from `ix` rather than a `pkgs` callPackage formal
  # (which `override` can't reach); `ix.pkgs` is the caller's set.
  inherit (ix) pkgs;

  # mix.exs declares `~> 1.18`; same toolchain pairing as the consumer
  # (packages/mcp-ex), so the path dep never compiles under a different
  # Elixir than the one that gated it here.
  elixir = ix.languages.elixir.toolchain pkgs {version = "1.19";};
  erlang = ix.languages.erlang.toolchain pkgs {version = "28";};

  version = "0.1.0"; # keep in sync with mix.exs

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./lib
      ./test
      ./mix.exs
      ./mix.lock
      ./.formatter.exs
    ];
  };

  # Test-env mix deps (credo + its deps) as a fixed-output derivation so the
  # sandboxed check runs offline; the library itself has zero runtime deps.
  # The SRI pin lives in the sibling pins.json (repo policy: no inline hash
  # literals); it has no URL (the FOD content is derived from mix.lock), so
  # refresh it after a lock change by building and copying the `got:` hash
  # from the mismatch error.
  mixFodDeps = pkgs.beamPackages.fetchMixDeps {
    pname = "agent-harness-ex-deps";
    inherit version elixir;
    src = lib.fileset.toSource {
      root = ./.;
      fileset = lib.fileset.unions [
        ./mix.exs
        ./mix.lock
      ];
    };
    mixEnv = "test";
    inherit ((ix.pins.loadPins ./pins.json).mix-deps) hash;
  };

  # The required Elixir quality lane: compile --warnings-as-errors (Elixir
  # 1.18's set-theoretic type checker), format, `mix credo --strict` against
  # the shared lib/elixir/credo.exs, and the ExUnit suite covering the
  # harness semantics (immediate spawn, checkpoint delivery, blocking wait,
  # slot accounting, caps, budget).
  elixirCheck = ix.buildElixirCheck pkgs {
    pname = "agent-harness-ex-check";
    inherit version src elixir erlang;
    mixDeps = mixFodDeps;
  };

  meta = {
    description = "The Fable 5 async-subagents harness (system card sec 8.15.3) as an OTP library: agents as processes, mailboxes as Send/Wait";
    license = lib.licenses.mit;
  };

  # The package output is the compiled :agent_harness OTP app, the same
  # shape tui-ex ships, so a release or an ERL_LIBS path can load it without
  # mix. ix-mcp-ex consumes the *source* as a mix path dependency instead;
  # this artifact exists so the library builds standalone and stays cached.
  package = pkgs.stdenv.mkDerivation {
    pname = "agent-harness-ex";
    inherit version src meta;
    strictDeps = true;
    # Mix >= 1.18 starts Mix.PubSub, which opens a loopback TCP socket at
    # compile time; the darwin sandbox denies plain sockets without this.
    __darwinAllowLocalNetworking = true;

    # hex provides the SCM module Mix needs to parse the lockfile (which
    # names the test-only credo dep even though prod never compiles it).
    nativeBuildInputs = [
      erlang
      elixir
      (pkgs.beamPackages.hex.override {inherit elixir;})
    ];

    env = {
      # prod, so the test-only credo dep is not consulted: the library has
      # no runtime deps and this build never touches the network.
      MIX_ENV = "prod";
      HEX_OFFLINE = "1";
      LANG = "C.UTF-8";
      LC_CTYPE = "C.UTF-8";
    };

    postUnpack = ''
      export MIX_HOME="$TEMPDIR/mix"
      export HEX_HOME="$TEMPDIR/hex"
    '';

    buildPhase = ''
      # shell
      runHook preBuild
      mix compile --no-deps-check --warnings-as-errors
      runHook postBuild
    '';

    installPhase = ''
      # shell
      runHook preInstall
      mkdir -p "$out/lib"
      cp -R "_build/prod/lib/agent_harness" "$out/lib/agent_harness"
      runHook postInstall
    '';
  };
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests = {
          elixir = elixirCheck;
        };
      };
  })
