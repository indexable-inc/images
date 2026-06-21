{
  # The shared strict Credo config (lib/elixir/credo.exs), injected by
  # lib/default.nix so every Elixir gate enforces the same policy. The Elixir
  # counterpart of how `ruffAnnArgs` is threaded into buildUvApplication.
  credoConfig,
}:

/**
  Build the required Elixir quality-lane check derivation, shared by every
  repo-owned Elixir package so the lane is defined once.

  The lane mirrors what each package's standalone CI ran, now sandboxed and
  reproducible:
    1. `mix compile --warnings-as-errors` — the Elixir 1.18+ set-theoretic type
       checker runs here, so type findings (and any warning) fail the build.
    2. `mix format --check-formatted` — formatting is enforced, not advisory.
    3. `mix credo --strict` against the shared `credoConfig` — the static-analysis
       gate (security/perf/modernization/readability), single source of truth.
    4. `mix test` — the package's ExUnit suite.

  The result is meant to be attached as `passthru.tests.elixir` on the package
  and wired into `checks` through lib/per-system.nix, exactly as before.

  Arguments:
  - `pkgs`: nixpkgs instance.
  - `pname`, `version`: derivation identity.
  - `src`: the package source (a fileset toSource).
  - `elixir`, `erlang`: the toolchain pair the package builds against; the launcher
    must use the same pairing so a deploy never runs code the gate did not.
  - `sourceRoot`: optional stdenv sourceRoot when the mix project is nested under
    the src (e.g. "source/elixir").
  - `mixDeps`: a `beamPackages.fetchMixDeps` result for packages with hex deps;
    `null` for a dependency-free project (offline `mix` against source alone).
  - `extraEnv`: extra derivation env (e.g. MIX_REBAR for rebar-built deps, or
    LD_LIBRARY_PATH for a precompiled NIF).
  - `extraNativeBuildInputs`: extra packages on PATH for the build.
  - `setupHook`: extra shell appended to `postUnpack` (e.g. seeding an
    elixir_make artifact cache to short-circuit a NIF download in the sandbox).
*/
pkgs:
{
  pname,
  version ? "0.0.0",
  src,
  elixir,
  erlang,
  sourceRoot ? null,
  mixDeps ? null,
  extraEnv ? { },
  extraNativeBuildInputs ? [ ],
  setupHook ? "",
}:
let
  inherit (pkgs) lib;
  hasDeps = mixDeps != null;
  # With vendored deps present, mix must not try to re-resolve them online.
  depsCheckFlag = lib.optionalString hasDeps "--no-deps-check";
in
pkgs.stdenv.mkDerivation {
  inherit pname version src;
  sourceRoot = if sourceRoot != null then sourceRoot else null;

  strictDeps = true;

  nativeBuildInputs = [
    erlang
    elixir
    pkgs.git
  ]
  # hex is only needed when compiling fetched deps; a no-deps build skips it.
  ++ lib.optional hasDeps (pkgs.beamPackages.hex.override { inherit elixir; })
  ++ extraNativeBuildInputs;

  env = {
    MIX_ENV = "test";
    HEX_OFFLINE = "1";
    LANG = "C.UTF-8";
    LC_CTYPE = "C.UTF-8";
  }
  // extraEnv;

  postUnpack = ''
    export MIX_HOME="$TEMPDIR/mix"
    export HEX_HOME="$TEMPDIR/hex"
  ''
  + lib.optionalString hasDeps ''
    export MIX_DEPS_PATH="$TEMPDIR/deps"
    cp --no-preserve=mode -R "${mixDeps}" "$MIX_DEPS_PATH"
  ''
  + setupHook;

  configurePhase = ''
    runHook preConfigure
  ''
  + lib.optionalString hasDeps ''
    mix deps.compile --no-deps-check --skip-umbrella-children
    ln -s "$MIX_DEPS_PATH" ./deps
  ''
  + ''
    # The shared Credo policy is injected here rather than committed per-package,
    # so the gate cannot drift between packages.
    cp ${credoConfig} .credo.exs
    runHook postConfigure
  '';

  buildPhase = ''
    # shell
    runHook preBuild
    mix compile ${depsCheckFlag} --warnings-as-errors
    runHook postBuild
  '';

  doCheck = true;
  checkPhase = ''
    # shell
    runHook preCheck
    mix format --check-formatted
    mix credo --strict
    mix test ${depsCheckFlag}
    runHook postCheck
  '';

  installPhase = ''
    # shell
    runHook preInstall
    mkdir -p "$out"
    runHook postInstall
  '';
}
