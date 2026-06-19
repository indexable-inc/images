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
  claudeHooks,
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

  # Session-digest hook net: absent/empty digests stay silent (exit 0, no
  # output, so a host without the ix-context-digest timer loses nothing), a
  # present digest rides additionalContext verbatim, and an oversized digest
  # is hard-capped at 6000 chars.
  mkdir -p digest-home/.cache/ix
  if got="$(HOME="$PWD/no-such-home" ${claudeHooks}/bin/claude-hooks session-digest </dev/null)" && [ -z "$got" ]; then :; else
    printf 'session-digest hook check failed (missing digest): expected silent exit 0, got:\n%s\n' "$got" >&2
    exit 1
  fi
  printf 'Distilled lesson: prefer rg over grep.' > digest-home/.cache/ix/context-digest.md
  got="$(HOME="$PWD/digest-home" ${claudeHooks}/bin/claude-hooks session-digest </dev/null)"
  want='{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"Distilled lesson: prefer rg over grep."}}'
  if [ "$got" != "$want" ]; then
    printf 'session-digest hook check failed (digest present)\nexpected:\n%s\ngot:\n%s\n' "$want" "$got" >&2
    exit 1
  fi
  printf 'x%.0s' $(seq 9000) > digest-home/.cache/ix/context-digest.md
  cap="$(HOME="$PWD/digest-home" ${claudeHooks}/bin/claude-hooks session-digest </dev/null \
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
      if ! got="$(printf '%s' "$input" | HOME="$PWD/no-home" ${claudeHooks}/bin/claude-hooks prompt-priors)" \
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
      | CLAUDE_CODE_PRIMARY_CHECKOUTS="$primary" ${claudeHooks}/bin/claude-hooks worktree-guard)"
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
      CLAUDE_CODE_PRIMARY_CHECKOUTS="$primary" ${claudeHooks}/bin/claude-hooks worktree-guard)" ]; then
    printf 'worktree guard check failed: kill switch must allow silently\n' >&2
    exit 1
  fi

  runHook postInstallCheck
''
