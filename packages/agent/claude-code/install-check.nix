# Argv regression net for the launcher spec, run against a stub target so it is
# offline and instant. Guards the properties the wrapper exists for: injected
# flags ride BEFORE the user argv (subcommands keep parsing), every injected
# option-argument is one `=` token (nothing can swallow a positional), and
# `--settings` defers to a caller-provided one (the CLI is first-wins between
# two `--settings` flags). Drives the real generated spec with its `@helper@`
# target swapped for the stub, through the actual launcher binary (the built
# `$out/bin/${binName}` forces IX_LAUNCH_SPEC via makeBinaryWrapper `--set`, so
# the launcher is exercised directly here).
{
  lib,
  runtimeShell,
  ix,
  git,
  jq,
  repoPackages,
  hookRunner,
  launchSpec,
  settingsDefaultsFile,
  wrapperFlags,
}:
''
  runHook preInstallCheck

  launcher=${ix.rustWorkspace.units.binaries."config-launch"}/bin/config-launch
  stub="$PWD/stub"
  printf '%s\n' '#!${runtimeShell}' 'printf "%s\n" "$@"' > "$stub"
  chmod +x "$stub"
  sed "s|@helper@|$stub|" ${launchSpec} > "$PWD/test-spec.json"

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

  check "flags prepend; settings injected when caller passes none" \
    ${
      lib.escapeShellArg (
        lib.concatStringsSep "\n" (
          wrapperFlags
          ++ [
            "--settings=${settingsDefaultsFile}"
            "mcp"
            "list"
          ]
        )
      )
    } \
    mcp list

  check "caller --settings wins; package defaults stay out" \
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
  # one argv entry, ahead of the subcommand. Synthesize a spec with the two flags
  # appended to `flags` (mirrors what `map (d: "--add-dir=${"\${d}"}") addDirs`
  # produces in the wrapper) and assert they land between the baked flags and the
  # injected `--settings`.
  ${lib.getExe jq} '.flags += ["--add-dir=/nix/store/sample-skills", "--plugin-dir=/nix/store/sample-plugin"]' \
    "$PWD/test-spec.json" > "$PWD/dirs-spec.json"
  dirs_got="$(IX_LAUNCH_SPEC="$PWD/dirs-spec.json" "$launcher" mcp list)"
  dirs_want=${
    lib.escapeShellArg (
      lib.concatStringsSep "\n" (
        wrapperFlags
        ++ [
          "--add-dir=/nix/store/sample-skills"
          "--plugin-dir=/nix/store/sample-plugin"
          "--settings=${settingsDefaultsFile}"
          "mcp"
          "list"
        ]
      )
    )
  }
  if [ "$dirs_got" != "$dirs_want" ]; then
    printf 'claude launcher argv check failed: add-dir/plugin-dir tokens\nexpected:\n%s\ngot:\n%s\n' \
      "$dirs_want" "$dirs_got" >&2
    exit 1
  fi

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
