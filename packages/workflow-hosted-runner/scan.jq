# No job in this repository runs on a GitHub-hosted runner. Every `runs-on`
# claims an ephemeral fleet runner from the ix-ci-dispatcher.
#
# WHY THIS IS AN ALLOW LIST AND NOT A DENY LIST, which is the whole design.
#
# The obvious guard refuses `ubuntu-latest`. That guard is worth very little,
# because the hosted family is open-ended and GitHub keeps extending it:
# `ubuntu-24.04`, `ubuntu-24.04-arm`, `macos-15`, `windows-2025`, a
# larger-runner group name chosen in repo settings, a runner-group label that
# resolves to hosted capacity. Every one of those is a way back in, and a deny
# list has to have predicted the spelling. So the rule here is inverted: a
# `runs-on` is accepted only if it is recognisably a fleet claim, and anything
# else is refused whether or not we have heard of it. The hosted families are
# still named, but only to produce a better sentence -- they are not what makes
# the refusal happen.
#
# The migration off hosted runners ran for months here and in indexable-inc/ix
# and then stalled with a handful of jobs left in each. That is the reason this
# file exists rather than one more cleanup commit: the class regenerates.
# indexable-inc/ix's gate-dispatch-guard.yml arrived carrying a fresh
# `ubuntu-latest` long after that repo's campaign finished, through a review
# that did not think to look at `runs-on`, which is precisely the failure a lint
# absorbs and a human does not.
#
# TWO SHAPES A `runs-on:` GREP CANNOT SEE, and both were live in this repository
# when this was written. A `workflow_call` input can carry a hosted default, as
# `runner-label` does in ci-budget.yml and ci-budget-read-only.yml, which puts a
# hosted runner in every caller that does not override it -- including
# indexable-inc/ix's own ci.yml, so a hosted job ran on every one of that
# repository's CI runs and no search of that repository could have found it. And
# a caller can pass a hosted label into a reusable workflow's parameter. Both are
# checked below, so the guard covers the DECISION rather than the one syntax
# that usually expresses it.
#
# Inputs: $workflow, the repo-relative path, for the message. $allow, an object
# whose values are the written reason an entry is exempt, keyed by what is being
# excused, one key shape per rule below:
#
#   "<workflow>:<job>"                 a job's own `runs-on`
#   "<workflow>:input:<name>"          a `workflow_call` input's default
#   "<workflow>:<job>:with:<name>"     a label passed at a call site
#
# An exception is a named line in the calling check with prose next to it, never
# a loosened pattern. Every entry is printed on each run, so an exception cannot
# quietly outlive the thing that justified it.
#
# ONE THING TO KNOW BEFORE MOVING A JOB HERE. The fleet PATH is bash, coreutils,
# dash, git, github-runner, tar, gzip, nix and unzip, so a converted job usually
# has to fetch `gh` or `jq`. Spell the installables LITERALLY
# (`nix build --no-link --print-out-paths nixpkgs#gh nixpkgs#jq`):
# indexable-inc/ix's nix/checks/workflow-nix-bootstrap.jq withdraws its nixpkgs-only exemption the moment an
# argument contains `$`, so an `ensure() { nix build "nixpkgs#$1"; }` helper
# turns the job into one that must also install bootstrap-index-nix.

# Every string a `runs-on` can resolve to, in either spelling GitHub accepts: a
# bare scalar, or a list of labels. `.. | strings` walks both, so the shape
# cannot be used to slip past the check.
def runner_strings:
  [(.["runs-on"] // empty) | .. | strings];

# A RUN-SCOPED claim on the ephemeral fleet. The dispatcher mints a runner per
# job keyed on this label (crates/ci/dispatcher/src/spawn.rs).
#
# Run scoping is checked, not just the `ix-ci-run-` prefix, and the difference
# is not pedantic. A static `ix-ci-run-something` label is not hosted, but it is
# not per-job either: it matches any idle fleet runner rather than the ephemeral
# one spawned for this job, so the job can land on a runner still holding
# another job's credentials. Bare `self-hosted` is the same defect in its widest
# form. Both are refused for that reason rather than for being GitHub's.
#
# Matched as the `format(...)` expression rather than a rendered value, because
# the expression is what is in the YAML.
def is_fleet_claim:
  test("ix-ci-run-\\{0\\}-\\{1\\}-")
  and test("github\\.run_id")
  and test("github\\.run_attempt");

# A reusable workflow whose label is the caller's to choose. Not exempt, merely
# checked somewhere else: `hosted_call_sites` below reads what callers pass, and
# `hosted_input_defaults` reads what they get when they pass nothing.
def is_caller_supplied:
  test("inputs\\.[A-Za-z_-]*runner[-_]?label");

# Named only to sharpen the message. Enforcement is the allow list above; this
# decides which sentence to print, never whether to refuse.
#
# `self-hosted` is in here for the message's sake even though it is not one of
# GitHub's images. It is refused by the run-scoping rule, not by this one.
def hosted_family:
  test("^(ubuntu|macos|windows)-"; "i") or . == "self-hosted";

def describe($label):
  if ($label | hosted_family)
  then "`\($label)` is a GitHub-hosted runner"
  elif ($label | test("ix-ci-run-"))
  then "`\($label)` names the fleet but is not run-scoped, so it matches any idle runner rather than this job's ephemeral one and can land on a runner holding another job's credentials"
  else "`\($label)` is not a run-scoped `ix-ci-run-` fleet claim"
  end;

# Jobs whose runs-on holds anything that is neither a fleet claim nor a caller's
# parameter. A job with no `runs-on` at all is a reusable-workflow call (`uses:`)
# and owns no runner decision, so it is not examined here.
def hosted_jobs($allow):
  [ .jobs // {}
    | to_entries[]
    | select(.value | has("runs-on"))
    | . as $job
    | ($job.value | runner_strings) as $labels
    | select($allow | has("\($workflow | split("/") | last):\($job.key)") | not)
    | ($labels | map(select((is_fleet_claim or is_caller_supplied) | not))) as $bad
    | select($bad | length > 0)
    | {job: $job.key, labels: $bad}
  ];

# `on.workflow_call.inputs.<name>.default`, for any input a job actually uses as
# its runs-on. This is the shape ci-budget.yml shipped: `default: ubuntu-latest`
# on a parameter that indexable-inc/ix's own ci.yml then consumed without
# overriding, so every ix CI run carried a hosted job that no grep of ix would
# ever find.
def hosted_input_defaults($allow):
  # `on:` is a string for `on: push`, so every step down to the input map is
  # type-guarded. jq raises on indexing a string, and a guard that dies on a
  # workflow it cannot parse is a guard that stops checking the rest of them.
  ( [ .jobs // {} | to_entries[] | .value | runner_strings[] ] | join(" ") ) as $all_labels
  | ( .["on"] | if type == "object" then .workflow_call else null end
      | if type == "object" then .inputs else null end
      | if type == "object" then . else {} end ) as $inputs
  | [ $inputs
      | to_entries[]
      | select(.value | type == "object")
      # Bound before the `test`, because `$all_labels | test(...)` evaluates its
      # argument with `.` set to $all_labels: reading `.key` in there indexes a
      # string and jq aborts the whole scan. Watched doing exactly that.
      | . as $entry
      | select($all_labels | test("inputs\\." + ($entry.key | gsub("-"; "[-_]"))))
      | select($allow | has("\($workflow | split("/") | last):input:\($entry.key)") | not)
      | select((.value.default // "") | type == "string")
      | select((.value.default // "") != "")
      | select((.value.default // "") | is_fleet_claim | not)
      | {input: .key, default: .value.default}
    ];

# What a caller hands a reusable workflow. The parameter is exempt where it is
# declared precisely because it is checked here.
def hosted_call_sites($allow):
  [ .jobs // {}
    | to_entries[]
    | . as $job
    | ($job.value["with"] // {})
    | to_entries[]
    | select(.key | test("runner[-_]?label"))
    | select((.value | type) == "string" and .value != "")
    | select(.value | is_fleet_claim | not)
    # Bound before the `has`, for the same reason as in hosted_input_defaults:
    # `$allow | has(...)` evaluates its argument with `.` set to $allow, where
    # `.key` is a silent null rather than an error, so the key would be built
    # wrong and every call site would look exempt.
    | . as $entry
    | select($allow | has("\($workflow | split("/") | last):\($job.key):with:\($entry.key)") | not)
    | {job: $job.key, input: $entry.key, value: $entry.value}
  ];

($allow | fromjson) as $allow
| hosted_jobs($allow) as $jobs
| hosted_input_defaults($allow) as $defaults
| hosted_call_sites($allow) as $calls
| if ($jobs | length) > 0
  then error("\($workflow): job \($jobs[0].job) does not claim a fleet runner -- \($jobs[0].labels | map(describe(.)) | join("; ")). Every job in this repository runs on an ix-ci-dispatcher runner: `runs-on: [\"${{ format('ix-ci-run-{0}-{1}-<name>', github.run_id, github.run_attempt) }}\"]`. If this job genuinely cannot, add it to `allowlist` in packages/workflow-hosted-runner/default.nix with the reason written next to it; do not widen the pattern.")
  elif ($defaults | length) > 0
  then error("\($workflow): the `workflow_call` input `\($defaults[0].input)` defaults to `\($defaults[0].default)`, which is not a fleet claim, and a job here uses that input as its `runs-on`. A default is what every caller that does not override it gets, including callers in other repositories, so a hosted default puts a hosted runner in runs nobody reading this repository would find. Default it to a dispatcher claim, as .github/workflows/update-flake-lock.yml does: `runs-on: [\"${{ inputs.runner-label || format('ix-ci-run-{0}-{1}-<name>', github.run_id, github.run_attempt) }}\"]`.")
  elif ($calls | length) > 0
  then error("\($workflow): job \($calls[0].job) passes `\($calls[0].input): \($calls[0].value)` to a reusable workflow, which is a runner choice made at the call site. Pass a fleet claim or pass nothing.")
  else true
  end
