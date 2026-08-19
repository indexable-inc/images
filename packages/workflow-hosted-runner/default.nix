/*
Every job in this repository runs on an ix-ci-dispatcher fleet runner, or the
build refuses the change. ./scan.jq carries the rules and the reasoning; the
short version is that the rule is an ALLOW list -- a `runs-on` must be
recognisably a fleet claim -- so a hosted spelling nobody predicted is refused
exactly like `ubuntu-latest`.

A PACKAGE RATHER THAN A CHECK, for the reason packages/ci-budget-policy states
about its own suites: `requiredGateRoots` carries the package closures, so this
builds on every pull request and every push, and a guard nothing references is a
guard nobody notices failing.

A guard nobody has watched fail is not a guard, so the scanner runs over
fixtures before it runs over `.github/workflows`, and the fixtures pin both
directions. Passing: the two spellings of a fleet claim (bare string and list),
a reusable workflow's `inputs.runner-label` parameter, a job with no `runs-on`
at all (a `uses:` caller owns no runner decision), an input default that is
itself a fleet claim, and an allow-listed entry under each of the three key
shapes. Refused: `ubuntu-latest`, a hosted spelling this file never names,
`macos-15`, `windows-2025`, bare `self-hosted`, a hosted label hidden in a list
beside a good one, a hosted `workflow_call` input default, a caller passing a
hosted label into a reusable workflow's parameter, and each allow-list key shape
under a neighbouring key that must not match.

`allowlist` is the only escape hatch and it is a named entry with prose, never a
loosened pattern. Its value is printed on every run so an exception cannot
quietly outlive the thing that justified it.
*/
{
  ix,
  lib,
  pkgs ? ix.pkgs,
}: let
  # The .github tree alone, not the repo root: this lint reads nothing else, and
  # rooting it at the root would rebuild it on every edit anywhere in the
  # repository. Same narrowing, and the same reason, as packages/ci-budget-policy.
  repo = "indexable-inc/index";
  githubRoot = ix.paths.root + "/.github";
  scanner = ./scan.jq;
  allowlist = {
    # ossf/scorecard-action is a CONTAINER action (`runs.using: docker`, image
    # ghcr.io/ossf/scorecard-action), so the runner has to hand the job a
    # working Docker daemon. The ix ephemeral runners are Nix-based and ship
    # none, so dispatching this would not degrade the scan, it would fail the
    # step outright. Revisit when scorecard ships a non-container entrypoint.
    "scorecard.yml:analysis" = "ossf/scorecard-action is a docker container action and the Nix runners have no Docker daemon";

    # Fires on fork-authored pull requests via pull_request_target and does
    # nothing but close them with a comment: no checkout, no build, no fleet
    # resource it could want. Deliberately GitHub-hosted so refusing a
    # stranger's PR never waits on fleet capacity.
    "refuse-pull-requests.yml:refuse" = "closes fork PRs with a comment; no checkout, no build, deliberately GitHub-hosted so refusal never waits on fleet capacity";

    # The job the dispatcher is SUPPOSED to hand the ix-public push credential
    # to. Named rather than pattern-exempted, so the set of things holding a
    # fleet signing key is a list somebody can read.
    "cache-push.yml:push:credential" = "the ix-public push token is the point of this job; the shard index goes BEFORE the -cache-push suffix precisely so the grant still applies";

    # NOT a judgement that these should stay hosted. They should not, and the
    # gate names them on every run so the decision cannot be forgotten. What is
    # missing is a capacity call that is not this change's to make.
    #
    # `runner-label` defaults to `ubuntu-latest` in both reusable workflows, and
    # indexable-inc/ix's ci.yml calls ci-budget.yml without overriding it, so
    # that repository's `ci-budget` job is hosted on every CI run. It is also
    # `needs:` of the gate. Moving the default to a dispatcher claim therefore
    # puts a fleet claim on the critical path of every ix CI run, ahead of the
    # gate, on a pool with recorded starvation (indexable-inc/ix#7625: a
    # flake-check that waited 13096s for a slot). That trade is worth making and
    # it wants measuring first, plus a `-ci-budget` entry in the dispatcher's
    # small-class suffix table so the job does not reserve a medium envelope to
    # run a few seconds of Python.
    #
    # The shape to copy when it happens is update-flake-lock.yml, which made
    # exactly this move: `runs-on: ["${{ inputs.runner-label || format(...) }}"]`
    # with no hosted default. Its test was rewritten to guard that `runner-label`
    # still overrides rather than which runner the default names, and
    # .github/actions/ci-budget/test_ci_policy.py needs the same rewrite --
    # it currently asserts `"default: ubuntu-latest" in source`, so it pins the
    # defect in place.
    "ci-budget.yml:input:runner-label" = "hosted default on ix CI's critical path; needs a capacity call and a dispatcher small-class suffix first";
    "ci-budget-read-only.yml:input:runner-label" = "hosted default on ix CI's critical path; needs a capacity call and a dispatcher small-class suffix first";
  };
  allowJson = builtins.toJSON allowlist;
in
  pkgs.runCommandLocal "workflow-hosted-runner" {
    nativeBuildInputs = [pkgs.coreutils pkgs.jq pkgs.yq-go];
  } ''
    set -euo pipefail

    # One workflow at a time, so the cite in the message is a repo-relative path
    # rather than a store path nobody can open.
    scan() {
      yq -o=json '.' "$1" \
        | jq --exit-status --arg workflow "$2" --arg allow "$3" \
            --arg repo ${lib.escapeShellArg repo} --from-file ${scanner}
    }

    fixtures=$PWD/fixtures
    mkdir -p "$fixtures"
    failed=0
    ran=0

    # `expect <pass|refuse> <label> <file> [expected-substring] [allow-json]`.
    #
    # A refusal must also match the sentence it was supposed to produce. A guard
    # that refuses for the wrong reason -- a jq syntax slip, a missing `--arg` --
    # passes a bare "did it exit nonzero?" test while asserting nothing, which is
    # the failure mode ./workflow-guard-manifest.nix was written about.
    #
    # `set +e` around the capture, and `verdict` rather than `out` to hold it:
    # stdenv's builder runs under `set -e`, so an expected refusal would abort
    # the build before it could be judged, and a variable named `out` would
    # overwrite the output path this derivation has to create.
    expect() {
      want=$1
      label=$2
      file=$3
      want_msg=''${4:-}
      allow=''${5:-'{}'}
      ran=$((ran + 1))
      set +e
      verdict=$(scan "$fixtures/$file" "$file" "$allow" 2>&1)
      rc=$?
      set -e
      case "$want" in
      pass)
        if [ "$rc" -ne 0 ]; then
          printf 'workflow-hosted-runner: SELF-TEST FAILED (%s)\n  expected %s to pass, got exit %s:\n%s\n' \
            "$label" "$file" "$rc" "$verdict" >&2
          failed=1
        fi
        ;;
      refuse)
        if [ "$rc" -eq 0 ]; then
          printf 'workflow-hosted-runner: SELF-TEST FAILED (%s)\n  expected %s to be refused, it passed\n' \
            "$label" "$file" >&2
          failed=1
        elif ! printf '%s' "$verdict" | grep -qF "$want_msg"; then
          printf 'workflow-hosted-runner: SELF-TEST FAILED (%s)\n  expected %s to be refused saying `%s`, got exit %s:\n%s\n' \
            "$label" "$file" "$want_msg" "$rc" "$verdict" >&2
          failed=1
        fi
        ;;
      esac
    }

    # Both spellings of a fleet claim, a reusable workflow's parameter, and a
    # `uses:` caller, which owns no runner decision and must not be demanded to
    # make one.
    cat > "$fixtures/good.yml" <<'YML'
    jobs:
      as-a-list:
        runs-on: ["''${{ format('ix-ci-run-{0}-{1}-thing', github.run_id, github.run_attempt) }}"]
        steps:
          - run: echo hi
      parameterised:
        runs-on: ["''${{ inputs.runner-label || format('ix-ci-run-{0}-{1}-thing', github.run_id, github.run_attempt) }}"]
        steps:
          - run: echo hi
      calls-a-reusable-workflow:
        uses: ./.github/workflows/other.yml
    YML

    # An input default that is itself a fleet claim is the shape
    # update-flake-lock.yml moved to, and must keep passing.
    cat > "$fixtures/good-default.yml" <<'YML'
    "on":
      workflow_call:
        inputs:
          runner-label:
            type: string
            default: ""
    jobs:
      run:
        runs-on: ["''${{ inputs.runner-label || format('ix-ci-run-{0}-{1}-thing', github.run_id, github.run_attempt) }}"]
        steps:
          - run: echo hi
    YML

    # One job per file for the refusals: the scanner reports the first offender,
    # so a second bad job in the same file would never be reached and the
    # fixtures would pin one verdict while appearing to pin several.
    write_bad_job() {
      cat > "$fixtures/$1.yml" <<YML
    jobs:
      $1:
        runs-on: $2
        steps:
          - run: echo hi
    YML
    }
    write_bad_job ubuntu 'ubuntu-latest'
    # A spelling this repository has never carried and the jq file never names.
    # It is the whole reason the rule is an allow list: a deny list would have
    # had to predict it.
    write_bad_job unheard-of 'ubuntu-24.04-arm'
    write_bad_job macos 'macos-15'
    write_bad_job windows 'windows-2025'
    write_bad_job bare-self-hosted 'self-hosted'
    # A hosted label smuggled into a list beside a legitimate one. `.. | strings`
    # is what makes the shape irrelevant.
    write_bad_job mixed-list '["''${{ format(''''ix-ci-run-{0}-{1}-thing'''', github.run_id, github.run_attempt) }}", "ubuntu-latest"]'
    # Names the fleet, is not run-scoped. Not hosted, and refused anyway: it
    # matches any idle runner rather than this job's ephemeral one.
    write_bad_job static-label 'ix-ci-run-static-thing'
    # A hosted larger-runner group, in the {group,labels} shape `runs-on` also
    # accepts. Neither key is a fleet claim.
    cat > "$fixtures/runner-group.yml" <<'YML'
    jobs:
      runner-group:
        runs-on:
          group: my-larger-runners
          labels: [ubuntu-latest]
        steps:
          - run: echo hi
    YML

    # The ci-budget.yml shape: hosted only in the default of an input that a job
    # reads as its runs-on. Nothing in this file says `runs-on: ubuntu-latest`.
    cat > "$fixtures/hosted-default.yml" <<'YML'
    "on":
      workflow_call:
        inputs:
          runner-label:
            type: string
            default: ubuntu-latest
    jobs:
      run:
        runs-on: ''${{ inputs.runner-label }}
        steps:
          - run: echo hi
    YML

    # The other half of the same hole: the default is fine, and the caller
    # chooses hosted anyway.
    cat > "$fixtures/hosted-call-site.yml" <<'YML'
    jobs:
      call:
        uses: ./.github/workflows/reusable.yml
        with:
          runner-label: ubuntu-latest
    YML


    # A cross-repo call that hands over the runner decision by omission. This is
    # the ix ci.yml -> index ci-budget.yml shape.
    cat > "$fixtures/crossrepo-no-label.yml" <<'YML'
    jobs:
      budget:
        uses: someone-else/repo/.github/workflows/thing.yml@main
        with:
          pull-request-number: 1
    YML

    # The same call, made explicit. Must pass, or the rule is unsatisfiable and
    # the only way to comply is an allow list entry.
    cat > "$fixtures/crossrepo-with-label.yml" <<'YML'
    jobs:
      budget:
        uses: someone-else/repo/.github/workflows/thing.yml@main
        with:
          runner-label: "''${{ format('ix-ci-run-{0}-{1}-budget', github.run_id, github.run_attempt) }}"
    YML

    # A `uses:` naming THIS repository by full name, which index does. Local in
    # the way that matters: its default is readable by this same check.
    # Unquoted heredoc: the repo name has to be THIS repo's, so the fixture
    # tests the rule rather than a name that happens not to match.
    cat > "$fixtures/self-referencing-uses.yml" <<YML
    jobs:
      budget:
        uses: ${repo}/.github/workflows/thing.yml@main
    YML

    # POSITIVE CONTROL for the credential rule. Without this the rule could be
    # silently matching nothing and "no job grabs a credential" would be a
    # vacuous pass. `-cache-warm` is the exact trap this change nearly walked
    # into for cache-warm-reaper.yml.
    write_bad_job cache-warm-grab '["''${{ format(ix-ci-run-{0}-{1}-cache-warm, github.run_id, github.run_attempt) }}"]'

    # NEGATIVE CONTROL, and the whole point of the suffix discipline: extending
    # past the credential suffix must NOT trip the rule, or every job would need
    # an allow list entry and the rule would be abandoned.
    cat > "$fixtures/cache-warm-reaper.yml" <<'YML'
    jobs:
      reap:
        runs-on: ["''${{ format('ix-ci-run-{0}-{1}-cache-warm-reaper', github.run_id, github.run_attempt) }}"]
        steps:
          - run: echo hi
    YML

    expect pass "a run-scoped fleet claim, a parameter, and a \`uses:\` caller" good.yml
    expect pass "a workflow_call input defaulting to a fleet claim" good-default.yml
    expect refuse "\`ubuntu-latest\`" ubuntu.yml "is a GitHub-hosted runner"
    expect refuse "a hosted spelling this guard never names" unheard-of.yml "is a GitHub-hosted runner"
    expect refuse "\`macos-15\`" macos.yml "is a GitHub-hosted runner"
    expect refuse "\`windows-2025\`" windows.yml "is a GitHub-hosted runner"
    expect refuse "bare \`self-hosted\`, which is not the run-scoped label" bare-self-hosted.yml "is a GitHub-hosted runner"
    expect refuse "a hosted label in a list beside a legitimate fleet claim" mixed-list.yml "is a GitHub-hosted runner"
    expect refuse "a static \`ix-ci-run-\` label with no run scoping" static-label.yml "is not run-scoped"
    expect refuse "a hosted larger-runner group in the {group,labels} shape" runner-group.yml "is a GitHub-hosted runner"
    expect refuse "a hosted \`workflow_call\` input default" hosted-default.yml "defaults to \`ubuntu-latest\`"
    expect refuse "a caller passing a hosted label to a reusable workflow" hosted-call-site.yml "is a runner choice made at the call site"

    expect refuse "a cross-repo call that passes no runner-label" crossrepo-no-label.yml \
      "in another repository without passing"
    expect pass "a cross-repo call that passes a fleet claim" crossrepo-with-label.yml
    expect pass "a \`uses:\` naming this same repository, whose default this check can read" self-referencing-uses.yml
    expect refuse "a job whose label ENDS in a credential suffix" cache-warm-grab.yml \
      "grants a fleet-trust credential on exactly that suffix"
    expect pass "a label extending PAST the credential suffix" cache-warm-reaper.yml
    expect pass "a credential-bearing job named in the allow list" cache-warm-grab.yml "" \
      '{"cache-warm-grab.yml:cache-warm-grab:credential":"a fixture proving the credential key works"}'
    expect refuse "an allow list entry for a DIFFERENT job does not grant the credential" cache-warm-grab.yml \
      "grants a fleet-trust credential" '{"cache-warm-grab.yml:someone-else:credential":"wrong job"}'

    # The escape hatch, proved in both directions for each of the three key
    # shapes: exempt under its own key, still refused under a neighbouring key.
    # An allow list that matched too broadly would pass both halves of each
    # pair, and one that matched nothing would fail both.
    expect pass "an allow-listed job" ubuntu.yml "" '{"ubuntu.yml:ubuntu":"a fixture proving the escape hatch works"}'
    expect refuse "an allow list entry for a DIFFERENT job does not exempt this one" ubuntu.yml \
      "is a GitHub-hosted runner" '{"ubuntu.yml:somebody-else":"wrong job"}'
    expect pass "an allow-listed input default" hosted-default.yml "" \
      '{"hosted-default.yml:input:runner-label":"a fixture proving the input-default key works"}'
    expect refuse "the job key shape does not exempt an input default" hosted-default.yml \
      "defaults to \`ubuntu-latest\`" '{"hosted-default.yml:run":"wrong key shape"}'
    expect pass "an allow-listed call site" hosted-call-site.yml "" \
      '{"hosted-call-site.yml:call:with:runner-label":"a fixture proving the call-site key works"}'
    expect refuse "the job key shape does not exempt a call site" hosted-call-site.yml \
      "is a runner choice made at the call site" '{"hosted-call-site.yml:call":"wrong key shape"}'

    # "No fixture failed" is satisfied by no fixture having run, which is the
    # same absence-as-success shape guarded above. Pin the count so an `expect`
    # line deleted or lost to a shell typo is a red build rather than a quieter
    # green one. Bump this deliberately when adding a case.
    expected_assertions=25
    if [ "$ran" -ne "$expected_assertions" ]; then
      printf 'workflow-hosted-runner: %s fixture assertions ran, expected %s. A case was added or lost without updating the count, and "nothing failed" is not the same as "everything ran".\n' \
        "$ran" "$expected_assertions" >&2
      failed=1
    fi

    if [ "$failed" -ne 0 ]; then
      echo 'workflow-hosted-runner: the scanner does not agree with its own fixtures; fix ./scan.jq before trusting it over .github/workflows' >&2
      exit 1
    fi

    # An assertion whose passing state is an absence. If the glob ever stops
    # matching -- a moved directory, a renamed extension -- the loop below runs
    # zero times and this check reports success having read nothing. Count first.
    workflows=(${githubRoot}/workflows/*.yml)
    # 16 today: the 2026-08-18 projection amendment deleted the three
    # self-mutating updater lanes and approve-user-change, and moved
    # refuse-pull-requests.yml in from the deleted index-project overlay.
    # The floor is a staleness guard, not a census (matches the ix-side
    # check's minWorkflows = 14 in nix/checks/lint.nix).
    if [ ''${#workflows[@]} -lt 14 ]; then
      printf 'workflow-hosted-runner: found only %s workflow(s) under .github/workflows; the scan glob has stopped matching and this check would pass without reading anything\n' \
        "''${#workflows[@]}" >&2
      exit 1
    fi

    for workflow in "''${workflows[@]}"; do
      # stdout is jq's `true` per compliant workflow, 51 lines of it. The
      # refusal goes to stderr, so dropping stdout loses nothing diagnostic.
      scan "$workflow" ".github/workflows/$(basename "$workflow")" ${lib.escapeShellArg allowJson} >/dev/null
    done

    ${lib.concatStringsSep "\n" (
      lib.mapAttrsToList
      (key: why: ''printf 'workflow-hosted-runner: ALLOWED %s -- %s\n' ${lib.escapeShellArg key} ${lib.escapeShellArg why}'')
      allowlist
    )}
    printf 'workflow-hosted-runner: every job claims an ix-ci-run-* fleet runner\n'
    touch "$out"
  ''
