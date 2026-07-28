# Argv regression net for the launcher spec, run against a stub target so it is
# offline and instant. Guards the properties the wrapper exists for: injected
# flags ride BEFORE the user argv of a session, they are WITHHELD entirely when
# the first argument selects a subcommand (index#4269: the CLI re-reads the raw
# argv by fixed index once a subcommand is chosen, so a prepended flag reaches
# its parser and it exits `Unknown argument`), every injected option-argument is
# one `=` token (nothing can swallow a positional), and no settings ride argv
# (#3180: the render materializes into the writable user settings layer).
# Drives the real generated spec with its `@helper@`
# target swapped for the stub, through the actual launcher binary (the built
# `$out/bin/${binName}` forces IX_LAUNCH_SPEC via makeBinaryWrapper `--set`, so
# the launcher is exercised directly here).
{
  lib,
  runtimeShell,
  ix,
  git,
  jq,
  nushell,
  repoPackages,
  hookRunner,
  launchSpec,
  settingsDefaultsFile,
  wrapperFlags,
  subcommands,
  wrapperEnvDefaults,
  featureSettingsEnv,
  houseSettingsRender,
  statuslineCommand,
  disabledSystemTools,
  python3,
  binName,
}: ''
    runHook preInstallCheck

    launcher=${ix.rustWorkspace.units.binaries.config-launch}/bin/config-launch
    stub="$PWD/stub"
    printf '%s\n' '#!${runtimeShell}' 'printf "%s\n" "$@"' > "$stub"
    chmod +x "$stub"
    sed "s|@helper@|$stub|" ${launchSpec} > "$PWD/test-spec.json"

    spec_env() {
      ${lib.getExe jq} -r --arg key "$1" '.env[$key]' \
        "$PWD/test-spec.json"
    }

    if ! ${lib.getExe jq} -e \
      '.env.DISABLE_UPDATES == "1" and (.env | has("DISABLE_AUTOUPDATER") | not)' \
      "$PWD/test-spec.json" >/dev/null; then
      printf 'claude launcher env check failed: strict update disable is missing or legacy background-only disable remains\n' >&2
      exit 1
    fi

    skills_dir="$(spec_env IX_CLAUDE_SKILLS_DIR)"
    if [ ! -d "$skills_dir" ]; then
      printf 'claude launcher env check failed: IX_CLAUDE_SKILLS_DIR is not a directory: %s\n' \
        "$skills_dir" >&2
      exit 1
    fi
    ${lib.optionalString (repoPackages ? mcp-ex) ''
    agents_dir="$(spec_env IX_CLAUDE_AGENTS_DIR)"
    if [ ! -d "$agents_dir" ] || [ -z "$(find "$agents_dir" -maxdepth 1 -name '*.md' -print -quit)" ]; then
      printf 'claude launcher env check failed: IX_CLAUDE_AGENTS_DIR has no agent markdown files: %s\n' \
        "$agents_dir" >&2
      exit 1
    fi
  ''}

    # Every disabled typed feature (see the wrapper's `features` arg) must ride
    # the launch spec as an env_default, never plain env, so a caller export
    # can still re-enable it per session.
    check_env_default() {
      local key="$1" got
      got="$(${lib.getExe jq} -r --arg key "$key" \
        '.env_defaults[$key]' \
        "$PWD/test-spec.json")"
      if [ "$got" != 1 ]; then
        printf 'claude launcher env check failed: %s env_default is %s, want 1\n' \
          "$key" "$got" >&2
        exit 1
      fi
    }
    feature_envs=(${lib.escapeShellArgs (builtins.attrNames wrapperEnvDefaults)})
    for feature_env in "''${feature_envs[@]}"
    do
      check_env_default "$feature_env"
    done

    disabled_system_tools=(${lib.escapeShellArgs disabledSystemTools})
    for tool in "''${disabled_system_tools[@]}"
    do
      if ! ${lib.getExe jq} -e --arg tool "$tool" '.permissions.deny | index($tool)' \
        ${settingsDefaultsFile} >/dev/null; then
        printf 'system tool deny check failed: %s is not denied in settings defaults\n' "$tool" >&2
        exit 1
      fi
    done

    if ${lib.getExe jq} -e --argjson names ${lib.escapeShellArg (builtins.toJSON (builtins.attrNames wrapperEnvDefaults))} \
      '.env | keys[] | select(. as $k | $names | index($k))' \
      "$PWD/test-spec.json"; then
      printf 'claude launcher env check failed: disabled-feature vars must be env_defaults, not env\n' >&2
      exit 1
    fi
    ${lib.optionalString (wrapperEnvDefaults ? CLAUDE_CODE_DISABLE_1M_CONTEXT) ''
    envstub="$PWD/envstub"
    printf '%s\n' '#!${runtimeShell}' 'printf "%s\n" "''${CLAUDE_CODE_DISABLE_1M_CONTEXT-unset}"' > "$envstub"
    chmod +x "$envstub"
    sed "s|@helper@|$envstub|" ${launchSpec} > "$PWD/env-spec.json"
    got="$(env -u CLAUDE_CODE_DISABLE_1M_CONTEXT IX_LAUNCH_SPEC="$PWD/env-spec.json" "$launcher")"
    if [ "$got" != 1 ]; then
      printf '1M-context guard check failed: unset caller env must get the default, got %s\n' "$got" >&2
      exit 1
    fi
    got="$(env CLAUDE_CODE_DISABLE_1M_CONTEXT= IX_LAUNCH_SPEC="$PWD/env-spec.json" "$launcher")"
    if [ "$got" != "" ]; then
      printf '1M-context guard check failed: caller re-enable (empty value) must win, got %s\n' "$got" >&2
      exit 1
    fi
  ''}

    # The typed feature render must land verbatim in the baked settings env
    # (read at CC startup even when the launch env is missing), and the house
    # posture layer (post-extraSettings merge, minus controlled keys) must
    # land key-for-key in the settings file. Both expectations are derived
    # from the same nix values that build the file, so they hold for
    # overridden builds too.
    if ! ${lib.getExe jq} -e --argjson want ${lib.escapeShellArg (builtins.toJSON featureSettingsEnv)} \
      '.env as $env | $want | to_entries | all($env[.key] == .value)' \
      ${settingsDefaultsFile} >/dev/null; then
      printf 'feature settings env check failed: want %s within .env of %s\n' \
        ${lib.escapeShellArg (builtins.toJSON featureSettingsEnv)} ${settingsDefaultsFile} >&2
      exit 1
    fi
    if ! ${lib.getExe jq} -e --argjson want ${lib.escapeShellArg (builtins.toJSON houseSettingsRender)} \
      '. as $doc | $want | to_entries | all($doc[.key] == .value)' \
      ${settingsDefaultsFile} >/dev/null; then
      printf 'house settings default check failed: want %s within %s\n' \
        ${lib.escapeShellArg (builtins.toJSON houseSettingsRender)} ${settingsDefaultsFile} >&2
      exit 1
    fi

    # House statusline, driven through the exact command string the house
    # defaults bake (so the store paths inside it are exercised; the render
    # check above proves the settings file carries it, extraSettings aside).
    # Offline by construction: the seeded run finds a fresh cache and never
    # fetches; the cold run's fetch fails in the sandbox and must degrade to a
    # plain version segment. The seeded latest (1.2.10 vs current 1.2.3) also
    # guards the numeric per-segment compare a string compare would get
    # backwards.
    statusline_cmd=${lib.escapeShellArg statuslineCommand}
    if [ "$(${lib.getExe nushell} --no-config-file -c "nu-check '${./statusline.nu}'")" != true ]; then
      printf 'statusline check failed: nu-check rejected statusline.nu\n' >&2
      exit 1
    fi
    statusline_payload='{"version":"1.2.3","model":{"display_name":"TestModel"},"context_window":{"context_window_size":200000,"current_usage":{"input_tokens":100000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}'
    mkdir -p sl-home/.cache/ix-claude-statusline sl-cold-home
    printf '1.2.10' > sl-home/.cache/ix-claude-statusline/latest
    got="$(printf '%s' "$statusline_payload" \
      | HOME="$PWD/sl-home" XDG_CACHE_HOME="$PWD/sl-home/.cache" ${runtimeShell} -c "$statusline_cmd")"
    case "$got" in
    *'⟡ 𝒊𝒙 | █████░░░░░ | TestModel | high | v1.2.3 '*'↑1.2.10'*) : ;;
    *)
      printf 'statusline check failed (seeded cache): want bar/model/effort/version/update marker, got:\n%s\n' "$got" >&2
      exit 1
      ;;
    esac
    got="$(printf '%s' "$statusline_payload" \
      | HOME="$PWD/sl-cold-home" XDG_CACHE_HOME="$PWD/sl-cold-home/.cache" ${runtimeShell} -c "$statusline_cmd")"
    case "$got" in
    *'↑'*)
      printf 'statusline check failed (cold cache): update marker must not render offline, got:\n%s\n' "$got" >&2
      exit 1
      ;;
    *'⟡ 𝒊𝒙 | █████░░░░░ | TestModel | high | v1.2.3'*) : ;;
    *)
      printf 'statusline check failed (cold cache): want plain version segment, got:\n%s\n' "$got" >&2
      exit 1
      ;;
    esac

    check() {
      local desc="$1" expected="$2"
      shift 2
      local got
      got="$(IX_LAUNCH_SPEC="$PWD/test-spec.json" "$launcher" "$@")"
      if [ "$got" != "$expected" ]; then
        printf 'claude launcher argv check failed: %s\nexpected:\n%s\ngot:\n%s\n' \
          "$desc" "$expected" "$got" >&2
        exit 1
      fi
    }

    # No settings ride argv (#3180: defaults materialize into the writable
    # user settings layer instead), so the launch is exactly the baked flags
    # followed by the caller's argv.
    check "flags prepend; no settings injected" \
      ${
    lib.escapeShellArg (
      lib.concatStringsSep "\n" (
        wrapperFlags
        ++ [
          "explain this repo"
        ]
      )
    )
  } \
      'explain this repo'

    # Every token the CLI dispatches positionally must reach it as argv[2],
    # with nothing of ours ahead of it.
    subcommand_tokens=(${lib.escapeShellArgs subcommands})
    for token in "''${subcommand_tokens[@]}"
    do
      check "no flags ahead of the $token subcommand" "$token" "$token"
    done
    check "no flags ahead of mcp serve" "$(printf 'mcp\nserve')" mcp serve

    # A prompt is not a subcommand, even when it starts with one's name: the
    # match is on the whole token, so the session keeps every flag.
    check "a prompt that starts with a subcommand name keeps the flags" \
      ${
    lib.escapeShellArg (
      lib.concatStringsSep "\n" (
        wrapperFlags
        ++ [
          "mcp is broken, fix it"
        ]
      )
    )
  } \
      'mcp is broken, fix it'

    check "caller --settings passes through untouched" \
      ${
    lib.escapeShellArg (
      lib.concatStringsSep "\n" (
        wrapperFlags
        ++ [
          "--settings=/dev/null"
          "-p"
          "hi"
        ]
      )
    )
  } \
      --settings=/dev/null -p hi

    # addDirs/pluginDirs render as single, prepended `=` tokens. `--add-dir` is
    # variadic in the CLI, so a space-form token would swallow the next positional
    # (proven against the real binary); this guards that the launcher keeps each as
    # one argv entry, ahead of the caller argv. Synthesize a spec with the two flags
    # appended to `flags` (mirrors what `map (d: "--add-dir=${"\${d}"}") addDirs`
    # produces in the wrapper) and assert they land between the baked flags and the
    # caller argv.
    ${lib.getExe jq} '.flags += ["--add-dir=/nix/store/sample-skills", "--plugin-dir=/nix/store/sample-plugin"]' \
      "$PWD/test-spec.json" > "$PWD/dirs-spec.json"
    dirs_got="$(IX_LAUNCH_SPEC="$PWD/dirs-spec.json" "$launcher" 'explain this repo')"
    dirs_want=${
    lib.escapeShellArg (
      lib.concatStringsSep "\n" (
        wrapperFlags
        ++ [
          "--add-dir=/nix/store/sample-skills"
          "--plugin-dir=/nix/store/sample-plugin"
          "explain this repo"
        ]
      )
    )
  }
    if [ "$dirs_got" != "$dirs_want" ]; then
      printf 'claude launcher argv check failed: add-dir/plugin-dir tokens\nexpected:\n%s\ngot:\n%s\n' \
        "$dirs_want" "$dirs_got" >&2
      exit 1
    fi

    # Real-binary smoke net for the diagnostic command: `doctor` is an interactive
    # terminal UI, so stdin redirected from /dev/null hangs and the command is not
    # a good exit-status contract. Drive it through a local PTY, assert the wrapper
    # reaches the diagnostic screen, then close the persistent UI process.
    mkdir -p doctor-home doctor-home/config doctor-home/cache doctor-home/state
    CLAUDE_DOCTOR_BIN="$out/bin/${binName}" \
      HOME="$PWD/doctor-home" \
      XDG_CONFIG_HOME="$PWD/doctor-home/config" \
      XDG_CACHE_HOME="$PWD/doctor-home/cache" \
      XDG_STATE_HOME="$PWD/doctor-home/state" \
      TERM=xterm-256color \
      ${lib.getExe python3} <<'PY'
  import os
  import pty
  import re
  import select
  import subprocess
  import sys
  import time

  master, slave = pty.openpty()
  env = os.environ.copy()
  proc = subprocess.Popen(
      [env["CLAUDE_DOCTOR_BIN"], "doctor"],
      cwd=os.getcwd(),
      env=env,
      stdin=slave,
      stdout=slave,
      stderr=slave,
      close_fds=True,
  )
  os.close(slave)

  chunks = []
  sent_enter = False
  start = time.time()
  deadline = time.time() + 60
  while time.time() < deadline:
      ready, _, _ = select.select([master], [], [], 0.2)
      if ready:
          try:
              data = os.read(master, 4096)
          except OSError:
              break
          if not data:
              break
          chunks.append(data)
      output = b"".join(chunks)
      if not sent_enter and (
          (b"Enter" in output and b"close" in output) or time.time() - start > 25
      ):
          os.write(master, b"\r\n")
          sent_enter = True
      plain_loop = re.sub(
          r"\x1b\[[0-?]*[ -/]*[@-~]", "", output.decode("utf-8", "replace")
      )
      compact_loop = re.sub(r"\s+", "", plain_loop)
      if "Running:native" in compact_loop and "Search:OK" in compact_loop:
          proc.terminate()
          try:
              proc.wait(timeout=2)
          except subprocess.TimeoutExpired:
              proc.kill()
              proc.wait()
          break
      if proc.poll() is not None:
          break

  if proc.poll() is None:
      proc.terminate()
      try:
          proc.wait(timeout=2)
      except subprocess.TimeoutExpired:
          proc.kill()
          proc.wait()

  os.close(master)
  raw = b"".join(chunks).decode("utf-8", "replace")
  plain = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", raw)
  compact = re.sub(r"\s+", "", plain)
  for needle in ("Running:native", "Search:OK"):
      if needle not in compact:
          sys.stderr.write(f"claude doctor check failed: missing {needle!r}\n")
          sys.stderr.write(plain[-4000:])
          sys.exit(1)
  PY

    # Session-digest hook net: absent/empty digests stay silent (exit 0, no
    # output, so a host without the ix-context-digest timer loses nothing), a
    # present digest rides additionalContext verbatim, and an oversized digest
    # is hard-capped at 6000 chars.
    mkdir -p digest-home/.cache/ix
    if got="$(HOME="$PWD/no-such-home" ${hookRunner}/bin/claude-hooks session-digest </dev/null)" && [ -z "$got" ]; then :; else
      printf 'session-digest hook check failed (missing digest): expected silent exit 0, got:\n%s\n' "$got" >&2
      exit 1
    fi
    printf 'Distilled lesson: prefer rg over grep.' > digest-home/.cache/ix/context-digest.md
    got="$(HOME="$PWD/digest-home" ${hookRunner}/bin/claude-hooks session-digest </dev/null)"
    want='{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"Distilled lesson: prefer rg over grep."}}'
    if [ "$got" != "$want" ]; then
      printf 'session-digest hook check failed (digest present)\nexpected:\n%s\ngot:\n%s\n' "$want" "$got" >&2
      exit 1
    fi
    printf 'x%.0s' $(seq 9000) > digest-home/.cache/ix/context-digest.md
    cap="$(HOME="$PWD/digest-home" ${hookRunner}/bin/claude-hooks session-digest </dev/null \
      | ${lib.getExe jq} -r '.hookSpecificOutput.additionalContext | length')"
    if [ "$cap" != 6000 ]; then
      printf 'session-digest hook check failed (cap): expected 6000 chars, got %s\n' "$cap" >&2
      exit 1
    fi
    ${lib.optionalString (repoPackages ? search) ''

    # Fail-open net for the prompt-priors hook: every skip path must exit 0
    # with NO output (anything else would block or pollute the prompt).
    # Offline by construction: each input is rejected by a pre-flight gate
    # (short, no fleet noun, no credential, malformed JSON) before the
    # network-touching search would run. HOME points at an empty dir so the
    # credential gate cannot find a real mgrep token.
    mkdir -p no-home
    hook_silent() {
      local desc="$1" input="$2" got
      if ! got="$(printf '%s' "$input" | HOME="$PWD/no-home" ${hookRunner}/bin/claude-hooks prompt-priors)" \
        || [ -n "$got" ]; then
        printf 'prompt-priors hook check failed (%s): expected silent exit 0, got:\n%s\n' \
          "$desc" "$got" >&2
        exit 1
      fi
    }
    hook_silent "short prompt skipped" '{"prompt":"fix this typo"}'
    hook_silent "no fleet noun skipped" \
      '{"prompt":"please rename this function to something clearer for readability"}'
    hook_silent "no credential fails open" \
      '{"prompt":"how do we deploy the fleet with colmena to every host"}'
    hook_silent "malformed payload fails open" 'not json'
  ''}

    # Behavioral net for the worktree guard: a real primary checkout plus a
    # linked worktree, built in the sandbox, with the protected-glob env
    # override pointed at the primary. The guard must judge only the TARGET
    # path (allow worktree and out-of-repo edits, deny primary-checkout edits
    # even when the payload cwd lies elsewhere) and honor its kill switch.
    # pwd -P: git resolves physical paths (`--show-toplevel`), so the paths
    # the checks compare and glob against must be physical too.
    checktop="$(pwd -P)"
    primary="$checktop/repos/primary"
    wt="$checktop/repos/wt"
    ${lib.getExe git} init -q "$primary"
    ${lib.getExe git} -C "$primary" -c user.email=ci@ix -c user.name=ci \
      commit -q --allow-empty -m init
    ${lib.getExe git} -C "$primary" worktree add -q "$wt" -b guard-check

    guard() {
      local desc="$1" expect="$2" input="$3" got verdict
      got="$(printf '%s' "$input" \
        | CLAUDE_CODE_PRIMARY_CHECKOUTS="$primary" ${hookRunner}/bin/claude-hooks worktree-guard)"
      case "$got" in
      ''') verdict=allow ;;
      *'"permissionDecision":"deny"'*) verdict=deny ;;
      *) verdict="unparsed: $got" ;;
      esac
      if [ "$verdict" != "$expect" ]; then
        printf 'worktree guard check failed (%s): expected %s, got %s\n' \
          "$desc" "$expect" "$verdict" >&2
        exit 1
      fi
    }

    guard "edit inside linked worktree" allow \
      "{\"tool_input\":{\"file_path\":\"$wt/a.txt\"}}"
    guard "edit inside primary checkout" deny \
      "{\"tool_input\":{\"file_path\":\"$primary/a.txt\"}}"
    guard "cd evasion: cwd elsewhere, absolute target in primary" deny \
      "{\"cwd\":\"/tmp\",\"tool_input\":{\"file_path\":\"$primary/a.txt\"}}"
    guard "relative target resolves against payload cwd" deny \
      "{\"cwd\":\"$primary\",\"tool_input\":{\"file_path\":\"a.txt\"}}"
    guard "new file under unbuilt primary subdir" deny \
      "{\"tool_input\":{\"file_path\":\"$primary/new/deep/a.txt\"}}"
    guard "new file under unbuilt worktree subdir" allow \
      "{\"tool_input\":{\"file_path\":\"$wt/new/deep/a.txt\"}}"
    guard "edit outside any repo" allow \
      "{\"tool_input\":{\"file_path\":\"$checktop/repos/free.txt\"}}"
    guard "malformed payload fails open" allow 'not json'
    if [ -n "$(printf '%s' "{\"tool_input\":{\"file_path\":\"$primary/a.txt\"}}" \
      | CLAUDE_CODE_DISABLE_WORKTREE_GUARD=1 \
        CLAUDE_CODE_PRIMARY_CHECKOUTS="$primary" ${hookRunner}/bin/claude-hooks worktree-guard)" ]; then
      printf 'worktree guard check failed: kill switch must allow silently\n' >&2
      exit 1
    fi

    # PreToolUse guards (cargo-guard, bash-habits-guard, search-guard): a shared
    # deny/allow asserter on the JSON permissionDecision channel.
    pre_guard() {
      local sub="$1" desc="$2" expect="$3" input="$4" got verdict
      got="$(printf '%s' "$input" | ${hookRunner}/bin/claude-hooks "$sub")"
      case "$got" in
      ''') verdict=allow ;;
      *'"permissionDecision":"deny"'*) verdict=deny ;;
      *) verdict="unparsed: $got" ;;
      esac
      if [ "$verdict" != "$expect" ]; then
        printf '%s check failed (%s): expected %s, got %s\n' "$sub" "$desc" "$expect" "$verdict" >&2
        exit 1
      fi
    }

    pre_guard cargo-guard "bare cargo in monorepo denied" deny \
      '{"tool_name":"Bash","cwd":"/x/indexable-inc/ix","tool_input":{"command":"cargo test"}}'
    pre_guard cargo-guard "nix-wrapped cargo allowed" allow \
      '{"tool_name":"Bash","cwd":"/x/indexable-inc/ix","tool_input":{"command":"nix run .#run -- cargo test"}}'
    pre_guard cargo-guard "cargo outside monorepo allowed" allow \
      '{"tool_name":"Bash","cwd":"/tmp/other","tool_input":{"command":"cargo test"}}'
    pre_guard cargo-guard "non-Bash tool fails open" allow '{"tool_name":"Edit"}'
    pre_guard cargo-guard "malformed payload fails open" allow 'not json'

    pre_guard bash-habits-guard "stderr to /dev/null denied" deny \
      '{"tool_name":"Bash","tool_input":{"command":"make 2>/dev/null"}}'
    pre_guard bash-habits-guard "plain stdout /dev/null allowed" allow \
      '{"tool_name":"Bash","tool_input":{"command":"make >/dev/null"}}'
    pre_guard bash-habits-guard "recursive grep denied" deny \
      '{"tool_name":"Bash","tool_input":{"command":"grep -r foo ."}}'
    pre_guard bash-habits-guard "no-verify denied" deny \
      '{"tool_name":"Bash","tool_input":{"command":"git commit --no-verify"}}'
    pre_guard bash-habits-guard "quoted mention not a false positive" allow \
      "{\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"echo '2>/dev/null'\"}}"

    pre_guard search-guard "Search tool denied" deny '{"tool_name":"Search"}'
    pre_guard search-guard "WebSearch not denied" allow '{"tool_name":"WebSearch"}'

    # Behavioral net for git-guard (ENG-9964). Reuses the primary/worktree pair
    # built above, now with real uncommitted work in each: git has no
    # pre-reset/pre-clean/pre-stash hook, so PreToolUse is the only seam, and
    # the guard has to parse the command itself. These checks pin that parse.
    echo dirty > "$primary/tracked.txt"
    ${lib.getExe git} -C "$primary" add tracked.txt
    ${lib.getExe git} -C "$primary" -c user.email=ci@ix -c user.name=ci \
      commit -q -m tracked
    echo "another session's work" >> "$primary/tracked.txt"
    echo untracked > "$primary/scratch.txt"
    echo "my own work" >> "$wt/tracked.txt"

    git_payload() {
      ${lib.getExe jq} -nc --arg c "$1" --arg k "$2" \
        '{tool_name:"Bash",cwd:$c,tool_input:{command:$k}}'
    }
    git_guard() {
      local desc="$1" expect="$2" got verdict
      got="$(git_payload "$3" "$4" \
        | CLAUDE_CODE_PRIMARY_CHECKOUTS="$primary" IX_GIT=${lib.getExe git} \
          ${hookRunner}/bin/claude-hooks git-guard)"
      case "$got" in
      ''') verdict=allow ;;
      *'"permissionDecision":"deny"'*) verdict=deny ;;
      *) verdict="unparsed: $got" ;;
      esac
      if [ "$verdict" != "$expect" ]; then
        printf 'git-guard check failed (%s): expected %s, got %s\n' \
          "$desc" "$expect" "$verdict" >&2
        exit 1
      fi
    }

    # The five destructive shapes ENG-9964 names, in a dirty primary checkout.
    git_guard "reset --hard in primary"     deny "$primary" "git reset --hard HEAD~1"
    git_guard "checkout -- . in primary"    deny "$primary" "git checkout -- ."
    git_guard "clean -fd in primary"        deny "$primary" "git clean -fd"
    git_guard "stash in primary"            deny "$primary" "git stash"
    git_guard "restore . in primary"        deny "$primary" "git restore ."
    # Evasions that must not work: cd into the checkout, -C into it, a wrapper
    # command in front, a global option before the subcommand, and a
    # destructive call buried later in a chain.
    git_guard "cd evasion"                  deny /tmp "cd $primary && git reset --hard"
    git_guard "-C evasion"                  deny /tmp "git -C $primary clean -fdx"
    git_guard "sudo prefix"                 deny "$primary" "sudo git reset --hard"
    git_guard "buried in a chain"           deny "$primary" "make build && echo ok; git checkout -f main"
    git_guard "-c option before subcommand" deny "$primary" "git -c core.pager=cat reset --hard"
    # A linked worktree is the caller's own; their dirt is theirs to discard.
    git_guard "reset --hard in worktree"    allow "$wt" "git reset --hard HEAD~1"
    git_guard "clean -fd in worktree"       allow "$wt" "git clean -fd"
    # Mutating git in the primary checkout, none of which loses anything: the
    # hole index#4218 closed. `git add` and `git switch` through Bash are what
    # left the shared checkout on a deleted branch, 604 commits behind main,
    # with 534 files staged by nobody.
    git_guard "add in primary"              deny "$primary" "git add -A"
    git_guard "switch in primary"           deny "$primary" "git switch -c feature"
    git_guard "commit in primary"           deny "$primary" "git commit -am wip"
    git_guard "reset --soft in primary"     deny "$primary" "git reset --soft HEAD~1"
    git_guard "checkout -b in primary"      deny "$primary" "git checkout -b feature"
    git_guard "restore --staged"            deny "$primary" "git restore --staged ."
    git_guard "merge in primary"            deny "$primary" "git merge origin/main"
    git_guard "rebase in primary"           deny "$primary" "git rebase origin/main"
    # Read-only git in the primary checkout stays usable, or the guard becomes
    # noise and gets switched off.
    git_guard "status in primary"           allow "$primary" "git status"
    git_guard "log in primary"              allow "$primary" "git log --oneline -1"
    git_guard "diff in primary"             allow "$primary" "git diff"
    git_guard "rev-parse in primary"        allow "$primary" "git rev-parse HEAD"
    git_guard "ls-files in primary"         allow "$primary" "git ls-files"
    git_guard "branch listing in primary"   allow "$primary" "git branch -a"
    git_guard "worktree list in primary"    allow "$primary" "git worktree list"
    git_guard "clean -nd (dry run)"         allow "$primary" "git clean -nd"
    git_guard "add --dry-run"               allow "$primary" "git add --dry-run ."
    git_guard "stash list"                  allow "$primary" "git stash list"
    # The rescue commands the deny messages tell the caller to run must
    # themselves survive the guard, or the advice is a dead end.
    git_guard "stash create"                allow "$primary" "git stash create"
    git_guard "branch rescue/..."           allow "$primary" "git branch rescue/now"
    git_guard "worktree add"                allow "$primary" \
      "git worktree add /tmp/worktree/o/r/n -b b origin/main"
    # A literal mention inside a quoted string is not a command.
    git_guard "quoted mention"              allow "$primary" "echo 'git reset --hard'"
    git_guard "outside any protected repo"  allow /tmp "git reset --hard"
    if [ -n "$(printf '%s' "{\"tool_name\":\"Edit\",\"cwd\":\"$primary\"}" \
      | CLAUDE_CODE_PRIMARY_CHECKOUTS="$primary" ${hookRunner}/bin/claude-hooks git-guard)" ]; then
      printf 'git-guard check failed: non-Bash tool must allow silently\n' >&2
      exit 1
    fi

    # A clean protected checkout: nothing to lose, and the guard denies anyway
    # (index#4218). Reads there still pass, which is what proves the guard is
    # reading real repository state and not pattern-matching the string.
    clean="$checktop/repos/clean"
    ${lib.getExe git} init -q "$clean"
    ${lib.getExe git} -C "$clean" -c user.email=ci@ix -c user.name=ci \
      commit -q --allow-empty -m init
    clean_guard() {
      git_payload "$clean" "$1" \
        | CLAUDE_CODE_PRIMARY_CHECKOUTS="$clean" IX_GIT=${lib.getExe git} \
          ${hookRunner}/bin/claude-hooks git-guard
    }
    if [ -n "$(clean_guard "git status")" ]; then
      printf 'git-guard check failed: reads in a clean protected checkout must allow\n' >&2
      exit 1
    fi
    cleanmsg="$(clean_guard "git add -A")"
    case "$cleanmsg" in
    *'"permissionDecision":"deny"'*) : ;;
    *) printf 'git-guard check failed: git add in a clean protected checkout must deny, got: %s\n' \
        "$cleanmsg" >&2; exit 1 ;;
    esac
    # The refusal has to name the offending path and the worktree to use.
    for want in "$clean" "worktree add /tmp/worktree/" index#4218; do
      case "$cleanmsg" in
      *"$want"*) : ;;
      *) printf 'git-guard mutation message missing %s:\n%s\n' "$want" "$cleanmsg" >&2; exit 1 ;;
      esac
    done

    # The deny has to name what would be lost and where to go instead, or the
    # caller cannot act on it. Assert the message, not just the decision.
    msg="$(git_payload "$primary" "git reset --hard" \
      | CLAUDE_CODE_PRIMARY_CHECKOUTS="$primary" IX_GIT=${lib.getExe git} \
        ${hookRunner}/bin/claude-hooks git-guard)"
    for want in tracked.txt scratch.txt "worktree add" "stash create" ENG-9964; do
      case "$msg" in
      *"$want"*) : ;;
      *) printf 'git-guard message missing %s:\n%s\n' "$want" "$msg" >&2; exit 1 ;;
      esac
    done

    if [ -n "$(git_payload "$primary" "git reset --hard" \
      | CLAUDE_CODE_DISABLE_GIT_GUARD=1 CLAUDE_CODE_PRIMARY_CHECKOUTS="$primary" \
        IX_GIT=${lib.getExe git} ${hookRunner}/bin/claude-hooks git-guard)" ]; then
      printf 'git-guard check failed: kill switch must allow silently\n' >&2
      exit 1
    fi

    # Behavioral net for write-guard (index#4310). A subagent holding only
    # `Bash` wrote a module, a doc and a 55-line option into the shared
    # checkout through heredoc redirects, `cp` and `rm`: `worktree-guard`
    # judges an edit tool's `file_path` and never sees Bash, and `git-guard`
    # only classifies git. Reuses the same primary/worktree pair.
    write_guard() {
      local desc="$1" expect="$2" got verdict
      got="$(git_payload "$3" "$4" \
        | CLAUDE_CODE_PRIMARY_CHECKOUTS="$primary" IX_GIT=${lib.getExe git} \
          ${hookRunner}/bin/claude-hooks write-guard)"
      case "$got" in
      ''') verdict=allow ;;
      *'"permissionDecision":"deny"'*) verdict=deny ;;
      *) verdict="unparsed: $got" ;;
      esac
      if [ "$verdict" != "$expect" ]; then
        printf 'write-guard check failed (%s): expected %s, got %s\n' \
          "$desc" "$expect" "$verdict" >&2
        exit 1
      fi
    }

    # A multi-line payload is built with printf, never written inline: a
    # column-0 line inside this Nix indented string would reset the common
    # indentation prefix, re-indenting every other line of the script and
    # stranding the terminator of the python here-document further down.
    heredoc_doc="$(printf '%s\n' "cat > doc/note.md <<'EOF'" hello EOF)"
    heredoc_wt="$(printf '%s\n' "cat > a.txt <<'EOF'" hello EOF)"

    # The write vocabulary a shell command reaches for, aimed at the primary
    # checkout.
    write_guard "heredoc redirect"        deny "$primary" "$heredoc_doc"
    write_guard "append redirect"         deny "$primary" "echo x >> tracked.txt"
    write_guard "clobber-anyway redirect" deny "$primary" "echo x >| tracked.txt"
    write_guard "cp into primary"         deny "$primary" "cp /tmp/x tracked.txt"
    write_guard "mv out of primary"       deny "$primary" "mv tracked.txt /tmp/x"
    write_guard "rm in primary"           deny "$primary" "rm -f scratch.txt"
    write_guard "rm of the checkout"      deny /tmp "rm -rf $primary"
    write_guard "install into primary"    deny "$primary" "install -m 0644 /tmp/x doc/x"
    write_guard "tee into primary"        deny "$primary" "cat /tmp/x | tee tracked.txt"
    write_guard "sed -i in primary"       deny "$primary" "sed -i -e s/a/b/ tracked.txt"
    write_guard "truncate in primary"     deny "$primary" "truncate -s 0 tracked.txt"
    write_guard "patch reading stdin"     deny "$primary" "patch -p1 < /tmp/x.diff"
    write_guard "xargs rm from a pipe"    deny "$primary" "fd -e bak | xargs rm"
    # The evasions git-guard already has to handle, now for plain writes.
    write_guard "absolute target"         deny /tmp "echo x > $primary/tracked.txt"
    write_guard "cd evasion"              deny /tmp "cd $primary && echo x > tracked.txt"
    write_guard "sh -c wrapper"           deny /tmp "sh -c 'echo x > $primary/tracked.txt'"
    write_guard "buried in a chain"       deny "$primary" "make build && echo ok; rm tracked.txt"
    # Reads in the primary checkout stay usable, or the guard becomes noise and
    # gets switched off. Every one of these writes only OUTSIDE the checkout.
    write_guard "read piped out"          allow "$primary" "grep -n x tracked.txt > /tmp/hits"
    write_guard "sed filter to stdout"    allow "$primary" "sed -e s/a/b/ tracked.txt"
    write_guard "cp out of primary"       allow "$primary" "cp tracked.txt /tmp/backup"
    write_guard "stderr duplication"      allow "$primary" "make > /tmp/out 2>&1"
    write_guard "quoted mention"          allow "$primary" "echo 'rm tracked.txt'"
    write_guard "unclassified command"    allow "$primary" "nix build .#claude-hooks"
    # git is git-guard's; a second refusal here would carry a second
    # prescription for one command.
    write_guard "git left to git-guard"   allow "$primary" "git apply /tmp/x.diff"
    # The caller's own linked worktree is theirs to write.
    write_guard "heredoc in worktree"     allow "$wt" "$heredoc_wt"
    write_guard "rm in worktree"          allow "$wt" "rm -f tracked.txt"
    write_guard "patch in worktree"       allow "$wt" "patch -p1 < /tmp/x.diff"
    write_guard "outside any repo"        allow /tmp "rm -rf /tmp/whatever"
    write_guard "empty command"           allow "$primary" ""
    # The refusal has to name the token that tripped it, the worktree to use,
    # and the switch to turn it off, or the caller cannot act on it.
    wmsg="$(git_payload "$primary" "echo x > doc/note.md" \
      | CLAUDE_CODE_PRIMARY_CHECKOUTS="$primary" IX_GIT=${lib.getExe git} \
        ${hookRunner}/bin/claude-hooks write-guard)"
    for want in "doc/note.md" "$primary" "worktree add <dir>" index#4310 \
      CLAUDE_CODE_DISABLE_WORKTREE_GUARD=1; do
      case "$wmsg" in
      *"$want"*) : ;;
      *) printf 'write-guard message missing %s:\n%s\n' "$want" "$wmsg" >&2; exit 1 ;;
      esac
    done
    # One policy, one switch: the same export that stands the typed-tool guard
    # down stands this one down too.
    if [ -n "$(git_payload "$primary" "rm -f tracked.txt" \
      | CLAUDE_CODE_DISABLE_WORKTREE_GUARD=1 CLAUDE_CODE_PRIMARY_CHECKOUTS="$primary" \
        IX_GIT=${lib.getExe git} ${hookRunner}/bin/claude-hooks write-guard)" ]; then
      printf 'write-guard check failed: kill switch must allow silently\n' >&2
      exit 1
    fi

    # Review pair: log-edit records an edited path, the Stop gate then blocks once
    # (JSON decision:block) and consumes the marker; a stop_hook_active re-entry
    # allows silently (the loop guard).
    rstate="$PWD/review-state"
    printf '%s' '{"session_id":"s1","tool_input":{"file_path":"/a/b.rs"}}' \
      | CLAUDE_REVIEW_STATE_DIR="$rstate" ${hookRunner}/bin/claude-hooks review-log-edit
    gate="$(printf '%s' '{"session_id":"s1"}' \
      | CLAUDE_REVIEW_STATE_DIR="$rstate" ${hookRunner}/bin/claude-hooks review-gate)"
    case "$gate" in
    *'"decision":"block"'*) : ;;
    *) printf 'review-gate check failed: expected decision:block, got:\n%s\n' "$gate" >&2; exit 1 ;;
    esac
    again="$(printf '%s' '{"session_id":"s1"}' \
      | CLAUDE_REVIEW_STATE_DIR="$rstate" ${hookRunner}/bin/claude-hooks review-gate)"
    if [ -n "$again" ]; then
      printf 'review-gate check failed: consumed marker should allow silently, got:\n%s\n' "$again" >&2
      exit 1
    fi
    loop="$(printf '%s' '{"session_id":"s1","stop_hook_active":true}' \
      | CLAUDE_REVIEW_STATE_DIR="$rstate" ${hookRunner}/bin/claude-hooks review-gate)"
    if [ -n "$loop" ]; then
      printf 'review-gate check failed: stop_hook_active must allow silently, got:\n%s\n' "$loop" >&2
      exit 1
    fi

    # friction-report self-gates on an ix-contributor git author; the sandbox has
    # no git identity, so it must exit 0 silently (never block Stop, never file).
    if [ -n "$(printf '%s' '{"session_id":"s1","transcript_path":"/dev/null"}' \
      | HOME="$PWD/no-home" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null \
        ${hookRunner}/bin/claude-hooks friction-report)" ]; then
      printf 'friction-report check failed: non-contributor must exit silently\n' >&2
      exit 1
    fi

    # session-banner is best-effort host introspection; assert only that it never
    # crashes (fails open) on a minimal HOME.
    HOME="$PWD/no-home" ${hookRunner}/bin/claude-hooks session-banner </dev/null >/dev/null

    # Fail-open net for the subagent-cache hooks (ENG-4665): every skip and
    # error path must exit 0 with NO output (a lookup that emits would block the
    # Agent launch; a noisy populate would surface on every SubagentStop).
    # SUBAGENT_CACHE_URL points at a closed port so the one path that does reach
    # the network (a cacheable lookup) gets a refused connection and falls open.
    sac() {
      local desc="$1" sub="$2" input="$3" got
      got="$(printf '%s' "$input" \
        | SUBAGENT_CACHE_URL=http://127.0.0.1:1 ${hookRunner}/bin/claude-hooks "$sub")"
      if [ -n "$got" ]; then
        printf 'subagent-cache %s check failed (%s): expected silent, got:\n%s\n' \
          "$sub" "$desc" "$got" >&2
        exit 1
      fi
    }
    sac "malformed payload" subagent-cache-lookup 'not json'
    sac "missing fields" subagent-cache-lookup '{"tool_input":{}}'
    sac "non-cacheable agent skipped" subagent-cache-lookup \
      '{"tool_input":{"subagent_type":"general-purpose","prompt":"how does X work"}}'
    sac "cacheable lookup, daemon unreachable" subagent-cache-lookup \
      '{"tool_input":{"subagent_type":"explore","prompt":"how does X work"}}'
    sac "populate malformed payload" subagent-cache-populate 'not json'
    sac "populate missing transcript" subagent-cache-populate \
      '{"agent_type":"explore","last_assistant_message":"x","agent_transcript_path":"/no/such/transcript"}'
    if [ -n "$(printf '%s' '{"tool_input":{"subagent_type":"explore","prompt":"how does X work"}}' \
      | CLAUDE_CODE_DISABLE_SUBAGENT_CACHE=1 \
        SUBAGENT_CACHE_URL=http://127.0.0.1:1 ${hookRunner}/bin/claude-hooks subagent-cache-lookup)" ]; then
      printf 'subagent-cache lookup check failed: kill switch must be silent\n' >&2
      exit 1
    fi

    runHook postInstallCheck
''
