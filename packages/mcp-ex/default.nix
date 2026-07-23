{
  lib,
  ix,
  # The sibling package set: the compiled :tui_ex and :gmail_ex OTP apps
  # (packages/tui/ex, packages/google/gmail/ex) ride into the release and
  # the check env as `IX_MCP_TUI_EX`/`IX_MCP_GMAIL_EX`, never as mix deps,
  # so the kernel and the NIF bindings ship independently.
  repoPackages,
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

  # The :agent_harness mix path dependency (mix.exs points at
  # ../agent-harness-ex): every builder below stages the sibling's source
  # next to the unpacked project so the relative path resolves in the
  # sandbox. Reached through the package registry (`.src` of the sibling's
  # derivation) so its fileset stays the single source of truth and no `../`
  # literal crosses package directories here. Tripwire: if agent-harness-ex
  # ever gains a hex dep, the mixFodDeps FOD content below changes while its
  # pins.json hash will not; bump the pin there.
  stageAgentHarness = ''
    cp --no-preserve=mode -R ${repoPackages.agent-harness-ex.src} "$NIX_BUILD_TOP/agent-harness-ex"
  '';

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
    # deps.get loads mix.exs of every dep, path deps included.
    postUnpack = stageAgentHarness;
    inherit ((ix.pins.loadPins ./pins.json).mix-deps) hash;
  };

  # exqlite's dep telemetry builds with rebar3; mix finds it via MIX_REBAR3
  # instead of trying to install one (impossible offline).
  rebar3Env.MIX_REBAR3 = lib.getExe pkgs.beamPackages.rebar3;

  # Where IxMcp.TuiLocal loads the compiled :tui_ex app from at runtime
  # ($IX_MCP_TUI_EX/ebin goes on the code path, priv/ holds the NIF).
  tuiExApp = "${repoPackages.tui-ex}/lib/tui_ex";

  # Same pattern for IxMcp.Gmail: the compiled :gmail_ex app.
  gmailExApp = "${repoPackages.google-gmail-ex}/lib/gmail_ex";

  # The required Elixir quality lane: compile --warnings-as-errors (Elixir
  # 1.18's set-theoretic type checker), format, `mix credo --strict` against
  # the shared lib/elixir/credo.exs, and the ExUnit suite.
  elixirCheck = ix.buildElixirCheck pkgs {
    pname = "ix-mcp-ex-check";
    inherit version src elixir erlang;
    mixDeps = mixFodDeps;
    setupHook = stageAgentHarness;
    # IX_MCP_TUI_EX / IX_MCP_GMAIL_EX make the suite's NIF-binding tests
    # run in the sandbox (test_helper.exs skips them when unset).
    extraEnv =
      rebar3Env
      // {
        IX_MCP_TUI_EX = tuiExApp;
        IX_MCP_GMAIL_EX = gmailExApp;
      };
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
      ${stageAgentHarness}
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
      # PrWatch execs the GitHub CLI; baking gh's store path into IX_MCP_GH
      # keeps the watcher independent of whatever PATH the MCP client
      # launched the server with (#3553). set-default, not set: mix test and
      # operators may point the watcher at a different gh.
      # IX_MCP_TUI_EX follows the IX_MCP_GH pattern: bake the store path of
      # the compiled :tui_ex app so TuiLocal works regardless of the
      # client's PATH; set-default keeps it operator-overridable.
      makeWrapper "$out/lib/ix-mcp-ex/bin/ix_mcp" "$out/bin/ix-mcp-ex" \
        --set IX_MCP_STDIO 1 \
        --set RELEASE_DISTRIBUTION none \
        --set-default RELEASE_TMP /tmp \
        --set-default IX_MCP_GH ${lib.getExe pkgs.gh} \
        --set-default IX_MCP_TUI_EX ${tuiExApp} \
        --set-default IX_MCP_GMAIL_EX ${gmailExApp} \
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
      printf '%s\n%s\n%s\n%s\n%s\n%s\n%s\n%s\n%s\n%s\n%s\n' \
        '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}' \
        '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
        '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"session_set_name","arguments":{"name":"smoke"}}}' \
        '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"exec","arguments":{"intent":"utf8 wire roundtrip","budget":60,"code":"\"snow ☃\""}}}' \
        '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"exec","arguments":{"intent":"binary output rides as escapes","budget":60,"code":"IO.puts(<<255, 97>> <> \"bin-marker\")"}}}' \
        '{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"exec","arguments":{"intent":"connection survives binary output","budget":60,"code":"\"alive-after-binary\""}}}' \
        '{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"exec","arguments":{"intent":"crash dump routing probe","budget":60,"code":"System.fetch_env!(\"ERL_CRASH_DUMP\")"}}}' \
        '{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"exec","arguments":{"intent":"local pty drive probe","budget":60,"code":"{:ok, t} = TuiLocal.spawn(\"cat\", []); :ok = TuiLocal.send(t, \"pty-smoke\\r\"); {:ok, s} = TuiLocal.wait_for(t, \"pty-smoke\"); :ok = TuiLocal.close(t); s"}}}' \
        '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"exec","arguments":{"intent":"otp batteries present","budget":60,"code":"{:ok, _} = Application.ensure_all_started([:inets, :ssl, :xmerl, :runtime_tools, :tools]); \"otp-batteries-ok\""}}}' \
        '{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"exec","arguments":{"intent":"gmail nif runtime load probe","budget":60,"code":"false = Gmail.status().signed_in; \"gmail-nif-ok\""}}}' \
        '{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"exec","arguments":{"intent":"jason agent-compat probe","budget":60,"code":"%{\"k\" => 1} = Jason.decode!(~s({\"k\":1})); \"jason-compat-ok\""}}}' \
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
        *'"name":"exec"'*) ;;
        *)
          echo "tools/list did not advertise exec" >&2
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
      # Multi-byte UTF-8 in an inbound request used to kill the reader --
      # #3523: the :unicode io device made IO.binread/2 return
      # {:error, {:no_translation, :unicode, :latin1}}, swallowed as EOF.
      case "$out_lines" in
        *'snow ☃'*) ;;
        *)
          echo "exec did not round-trip a multi-byte UTF-8 payload" >&2
          printf '%s\n' "$out_lines" >&2
          exit 1
          ;;
      esac
      # Raw binary bytes in cell output used to kill the whole connection --
      # #3538: the {:error, ...} tuple from :unicode.characters_to_binary/1
      # poisoned the job record, no reply ever came, and the client hung.
      # The invalid byte must come back as a visible \xNN escape ("xFF"
      # rather than the backslash, which JSON escaping doubles) ...
      case "$out_lines" in
        *'xFFabin-marker'*) ;;
        *)
          echo "exec did not escape invalid bytes in cell output" >&2
          printf '%s\n' "$out_lines" >&2
          exit 1
          ;;
      esac
      # ... and the connection must survive to answer the next request.
      case "$out_lines" in
        *'alive-after-binary'*) ;;
        *)
          echo "connection did not survive binary cell output" >&2
          printf '%s\n' "$out_lines" >&2
          exit 1
          ;;
      esac
      # index#3539: the app exports ERL_CRASH_DUMP next to the action log, so
      # a BEAM crash dump cannot land in (and get committed to) whatever cwd
      # the MCP client ran the server from; the probe proves the routing
      # holds in the shipped release, not just under mix.
      case "$out_lines" in
        *"$PWD/erl_crash.dump"*) ;;
        *)
          echo "ERL_CRASH_DUMP is not routed next to the action log" >&2
          printf '%s\n' "$out_lines" >&2
          exit 1
          ;;
      esac
      # The release boots embedded (no code-path autoload), so this is the
      # one gate that proves IxMcp.TuiLocal's runtime load of the :tui_ex
      # app -- code path, app load, NIF @on_load -- works in the shipped
      # artifact, not just under mix.
      case "$out_lines" in
        *'pty-smoke'*) ;;
        *)
          echo "exec did not drive a local PTY through TuiLocal" >&2
          printf '%s\n' "$out_lines" >&2
          exit 1
          ;;
      esac
      # The Gmail sibling of the TuiLocal probe: proves IxMcp.Gmail's
      # runtime load of the :gmail_ex app (code path, app load, NIF
      # @on_load) works in the shipped release. The sandbox is signed out
      # by construction, so status() returning signed_in: false is also the
      # proof the auth state crosses as data, offline.
      case "$out_lines" in
        *'gmail-nif-ok'*) ;;
        *)
          echo "exec did not load the gmail NIF app through IxMcp.Gmail" >&2
          printf '%s\n' "$out_lines" >&2
          exit 1
          ;;
      esac
      # Agent-compat: jason rides the release purely so cells written from
      # Jason habit work; this proves the app is shipped and loadable in
      # the embedded release, not just present in mix.lock.
      case "$out_lines" in
        *'jason-compat-ok'*) ;;
        *)
          echo "exec could not call Jason.decode! in the release" >&2
          printf '%s\n' "$out_lines" >&2
          exit 1
          ;;
      esac
      # #3798: the release must carry the standard OTP batteries; this probe
      # proves :inets/:ssl/:xmerl/:runtime_tools/:tools boot in the shipped
      # release, not just in a full OTP install.
      case "$out_lines" in
        *'otp-batteries-ok'*) ;;
        *)
          echo "release is missing standard OTP applications" >&2
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
