{
  lib,
  ix,
}: let
  inherit (ix) pkgs;

  version = "0.1.0"; # keep in sync with mix.exs
  elixir = ix.languages.elixir.toolchain pkgs {version = "1.20";};
  erlang = ix.languages.erlang.toolchain pkgs {version = "29";};

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./lib
      ./test
      ./config
      ./mix.exs
      ./mix.lock
      ./.formatter.exs
    ];
  };

  mixFodDeps = pkgs.beamPackages.fetchMixDeps {
    pname = "loom-deps";
    inherit version src elixir;
    mixEnv = "test";
    inherit ((ix.pins.loadPins ./pins.json).mix-deps) hash;
  };

  elixirCheck = ix.buildElixirCheck pkgs {
    pname = "loom-check";
    inherit version src elixir erlang;
    mixDeps = mixFodDeps;
    extraNativeBuildInputs = [pkgs.bash];
    setupHook = ''
      patchShebangs "$sourceRoot/test/support/fake-ix"
    '';
  };

  ixCli = pkgs.callPackage ./ix-cli.nix {inherit ix;};

  package = pkgs.stdenv.mkDerivation {
    pname = "loom";
    inherit version src;
    strictDeps = true;
    __darwinAllowLocalNetworking = true;

    nativeBuildInputs = [
      erlang
      elixir
      (ix.elixirHex {inherit pkgs elixir;})
      pkgs.makeWrapper
    ];

    env = {
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
      runHook preBuild
      mix release --no-deps-check --path "$out/lib/loom"
      runHook postBuild
    '';

    installPhase = ''
      runHook preInstall
      makeWrapper "$out/lib/loom/bin/loom" "$out/bin/loom" \
        --set RELEASE_DISTRIBUTION none \
        --set-default RELEASE_TMP /tmp \
        --add-flags start_iex
      runHook postInstall
    '';

    meta = {
      description = "Snapshot-forked Claude subagents driven through ix VMs";
      license = lib.licenses.mit;
      mainProgram = "loom";
      platforms = ["x86_64-linux"];
    };
  };
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        inherit ixCli;
        tests.elixir = elixirCheck;
      };
  })
