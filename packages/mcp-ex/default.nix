{
  lib,
  ix,
}: let
  # Read the package set from `ix` rather than a `pkgs` callPackage formal
  # (which `override` can't reach); `ix.pkgs` is the caller's set.
  inherit (ix) pkgs;

  # mix.exs declares `~> 1.18`; the server and the quality gate build against
  # the same toolchain so the release never runs code the gate did not check.
  # 1.18 rather than 1.19: Mix 1.19's PubSub opens a loopback TCP socket at
  # compile time, which the darwin sandbox denies (:eperm).
  elixir = ix.languages.elixir.toolchain pkgs {version = "1.18";};
  erlang = ix.languages.erlang.toolchain pkgs {version = "27";};

  version = "0.1.0"; # keep in sync with mix.exs

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

  # Mix deps (exqlite + its build deps, plus test-only credo) as a
  # fixed-output derivation so the sandboxed builds run offline; mixEnv=test
  # is a superset of prod, so the release build stages the same FOD. The SRI
  # pin lives in the sibling pins.json (repo policy: no inline hash literals);
  # it has no URL (the FOD content is derived from mix.lock), so refresh it
  # after a lock change by building and copying the `got:` hash from the
  # mismatch error.
  mixFodDeps = pkgs.beamPackages.fetchMixDeps {
    pname = "ix-mcp-ex-deps";
    inherit version src elixir;
    mixEnv = "test";
    inherit ((ix.pins.loadPins ./pins.json).mix-deps) hash;
  };

  # exqlite's dep telemetry builds with rebar3; mix finds it via MIX_REBAR3
  # instead of trying to install one (impossible offline).
  rebar3Env.MIX_REBAR3 = lib.getExe pkgs.beamPackages.rebar3;

  # The required Elixir quality lane: compile --warnings-as-errors (Elixir
  # 1.18's set-theoretic type checker), format, `mix credo --strict` against
  # the shared lib/elixir/credo.exs, and the ExUnit suite.
  elixirCheck = ix.buildElixirCheck pkgs {
    pname = "ix-mcp-ex-check";
    inherit version src elixir erlang;
    mixDeps = mixFodDeps;
    extraEnv = rebar3Env;
  };

  meta = {
    description = "An MCP server whose REPL is Elixir: persistent bindings on a supervised BEAM evaluator";
    license = lib.licenses.mit;
    mainProgram = "ix-mcp-ex";
  };

  package = pkgs.stdenv.mkDerivation {
    pname = "ix-mcp-ex";
    inherit version src meta;
    strictDeps = true;
    # Mix >= 1.18 starts Mix.PubSub, which opens a loopback TCP socket at
    # compile time; the darwin sandbox denies plain sockets without this.
    __darwinAllowLocalNetworking = true;

    # hex provides the SCM module Mix needs to parse the lockfile and
    # compile the staged deps; nothing is fetched in the prod build.
    nativeBuildInputs = [
      erlang
      elixir
      (pkgs.beamPackages.hex.override {inherit elixir;})
      pkgs.makeWrapper
    ];

    env =
      {
        MIX_ENV = "prod";
        HEX_OFFLINE = "1";
        LANG = "C.UTF-8";
        LC_CTYPE = "C.UTF-8";
      }
      // rebar3Env;

    # The deps FOD is read-only in the store; mix wants a writable deps dir
    # (it compiles exqlite's NIF from vendored source there, forced by the
    # `:elixir_make, :force_build` config since the sandbox has no network).
    postUnpack = ''
      export MIX_HOME="$TEMPDIR/mix"
      export HEX_HOME="$TEMPDIR/hex"
      export MIX_DEPS_PATH="$TEMPDIR/deps"
      cp --no-preserve=mode -R "${mixFodDeps}" "$MIX_DEPS_PATH"
    '';

    buildPhase = ''
      # shell
      runHook preBuild
      # --no-deps-check keeps mix from trying to re-resolve the lock online;
      # the staged FOD above provides everything.
      mix deps.compile --no-deps-check --skip-umbrella-children
      mix release --no-deps-check --path "$out/lib/ix-mcp-ex"
      runHook postBuild
    '';

    installPhase = ''
      # shell
      runHook preInstall
      # The release launcher needs a writable RELEASE_TMP (the default is the
      # release root, which is the read-only store).
      # RELEASE_DISTRIBUTION=none: a stdio MCP server needs no Erlang
      # distribution, and the default sname mode wants an epmd listen socket
      # (denied in sandboxes, pointless everywhere else).
      makeWrapper "$out/lib/ix-mcp-ex/bin/ix_mcp" "$out/bin/ix-mcp-ex" \
        --set IX_MCP_STDIO 1 \
        --set RELEASE_DISTRIBUTION none \
        --set-default RELEASE_TMP /tmp \
        --add-flags start
      runHook postInstall
    '';
  };

  # End-to-end wire smoke test: a real MCP initialize -> tools/list ->
  # tools/call exchange over the installed binary's stdio, no network. The
  # action log needs a writable path (the sandbox HOME is not), so the env
  # override points it into the build dir, which also proves the SQLite NIF
  # loads in the release.
  smoke =
    pkgs.runCommand "ix-mcp-ex-smoke"
    {
      nativeBuildInputs = [package];
      strictDeps = true;
    }
    ''
      set +e
      printf '%s\n%s\n%s\n' \
        '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}' \
        '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
        '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"session_set_name","arguments":{"name":"smoke"}}}' \
        | IX_MCP_ACTIONS_DB="$PWD/actions.db" ix-mcp-ex > response.jsonl 2> server-stderr.log
      rc=$?
      set -e
      if [ "$rc" -ne 0 ]; then
        echo "ix-mcp-ex exited $rc" >&2
        echo "--- stderr ---" >&2
        cat server-stderr.log >&2
        echo "--- stdout ---" >&2
        cat response.jsonl >&2
        echo "--- env ---" >&2
        env >&2
        exit 1
      fi
      out_lines=$(cat response.jsonl)
      case "$out_lines" in
        *'"protocolVersion":"2025-06-18"'*) ;;
        *)
          echo "initialize did not negotiate the protocol version" >&2
          printf '%s\n' "$out_lines" >&2
          exit 1
          ;;
      esac
      case "$out_lines" in
        *'"elixir_exec"'*) ;;
        *)
          echo "tools/list did not advertise elixir_exec" >&2
          printf '%s\n' "$out_lines" >&2
          exit 1
          ;;
      esac
      case "$out_lines" in
        *'session named: smoke'*) ;;
        *)
          echo "tools/call session_set_name did not answer" >&2
          printf '%s\n' "$out_lines" >&2
          exit 1
          ;;
      esac
      if [ ! -s actions.db ]; then
        echo "action log was not written to actions.db" >&2
        exit 1
      fi
      mkdir -p "$out"
    '';
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests = {
          elixir = elixirCheck;
          inherit smoke;
        };
      };
  })
