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
  elixir = ix.languages.elixir.toolchain pkgs {version = "1.20";};
  erlang = ix.languages.erlang.toolchain pkgs {version = "29";};

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
    pname = "fleet-mesh-deps";
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

  # The required Elixir quality lane: compile --warnings-as-errors, format,
  # `mix credo --strict` against the shared lib/elixir/credo.exs, and the
  # ExUnit suite covering the engine semantics (snapshot on subscribe, edge
  # emission, unknown-on-failure, subscriber liveness, loud unconfigured
  # policy) plus the mesh client contract.
  elixirCheck = ix.buildElixirCheck pkgs {
    pname = "fleet-mesh-check";
    inherit version src elixir erlang;
    mixDeps = mixFodDeps;
  };

  meta = {
    description = "Fleet condition engine (snapshot + edge notifications behind a Policy seam) and the shared BEAM mesh client";
    license = lib.licenses.mit;
  };

  # The package output is the compiled :fleet_mesh OTP app, the same shape
  # agent-harness-ex ships, so a release or an ERL_LIBS path can load it
  # without mix. ix-mcp-ex consumes the *source* as a mix path dependency
  # instead; this artifact exists so the library builds standalone and stays
  # cached.
  package = pkgs.stdenv.mkDerivation {
    pname = "fleet-mesh";
    inherit version src meta;
    strictDeps = true;
    # MIX_ENV=prod compile of the library only: the ExUnit suite needs the
    # test-env deps this offline build deliberately omits.
    doCheck = false;
    # Mix >= 1.18 starts Mix.PubSub, which opens a loopback TCP socket at
    # compile time; the darwin sandbox denies plain sockets without this.
    __darwinAllowLocalNetworking = true;

    # hex provides the SCM module Mix needs to parse the lockfile (which
    # names the test-only credo dep even though prod never compiles it).
    nativeBuildInputs = [
      erlang
      elixir
      (ix.elixirHex {inherit pkgs elixir;})
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
      cp -R "_build/prod/lib/fleet_mesh" "$out/lib/fleet_mesh"
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
