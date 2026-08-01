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
# TWO SHAPES A `runs-on:` GREP CANNOT SEE, and both were live when this was
# written. A `workflow_call` input can carry a hosted default
# (`runner-label: {default: ubuntu-latest}`), which puts a hosted runner in
# every caller that does not override it -- including callers in other
# repositories, where nobody looking at this repo would find it. And a caller
# can pass a hosted label into a reusable workflow's parameter. Both are checked
# below, so the guard covers the DECISION rather than the one syntax that
# usually expresses it.
#
# Inputs: $workflow, the repo-relative path, for the message. $repo, this
# repository's `owner/name`, so a self-referencing `uses:` is recognised as
# local. $allow, an object
# whose values are the written reason an entry is exempt, keyed by what is being
# excused, one key shape per rule below:
#
#   "<workflow>:<job>"                 a job's own `runs-on`
#   "<workflow>:input:<name>"          a `workflow_call` input's default
#   "<workflow>:<job>:with:<name>"     a label passed at a call site
#   "<workflow>:<job>:uses"            a cross-repo call with no runner-label
#   "<workflow>:<job>:credential"      a job entitled to its credential suffix
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

# A CROSS-REPO reusable-workflow call that passes no `runner-label` at all.
# Ported from ix#9423, whose author found it; it is the other half of the
# input-default hole and this file did not have it.
#
# Not passing the label means trusting a default declared in a repository this
# check cannot read. That is not hypothetical: ix's own ci.yml calls
# `indexable-inc/index/.github/workflows/ci-budget.yml@main` passing only
# `pull-request-number` and `force-big-change`, and index defaults
# `runner-label` to `ubuntu-latest`, so a hosted job has been running on every
# ix CI run. Reading only `runs-on` certifies that tree clean.
#
# LOCAL calls are exempt, and getting this wrong is instructive. Requiring the
# label everywhere is unsatisfiable: auto-deploy.yml calls ./deploy.yml, which
# declares no `runner-label` input at all, and does not need one because its
# `runs-on` is in this tree and checked by this same guard. ix#9423 found that
# by looking at the callee rather than allow-listing the complaint.
def crossrepo_calls_without_label($allow):
  [ .jobs // {}
    | to_entries[]
    | . as $job
    | select($job.value.uses? != null)
    # Local is `./`, and also `<this repo>/...`, which index uses to call its own
    # reusable workflows by full name. The distinction that matters is not the
    # syntax but whether THIS check can read the default being relied on: it can
    # for its own repository (the input-default rule above does exactly that),
    # and it cannot for anyone else's.
    | select($job.value.uses | startswith("./") | not)
    | select($job.value.uses | startswith($repo + "/") | not)
    | select((($job.value["with"] // {}) | to_entries | map(select(.key | test("runner[-_]?label"))) | length) == 0)
    | select($allow | has("\($workflow | split("/") | last):\($job.key):uses") | not)
    | {job: $job.key, uses: $job.value.uses}
  ];

# A job whose label ENDS in a suffix the dispatcher grants fleet-trust
# credentials on, without being named as entitled to it.
#
# This is the trap that nearly caught this change: the obvious suffix for
# cache-warm-reaper.yml is `-cache-warm`, and `ix_nix_cache_push_eligible`
# matches on `label.ends_with("-cache-warm")`, so a job whose entire purpose is
# cancelling workflow runs would have been handed the fleet nix-cache SIGNING
# key. Checked mechanically here rather than left to whoever names the next job
# noticing, since the failure is silent and the blast radius is fleet-wide.
#
# The three suffixes are the `ends_with` sites in
# crates/ci/dispatcher/src/handler/workflow_job/credential_gate.rs.
def credential_suffixes: ["-cache-push", "-cache-warm", "-prod"];

def claim_name:
  (capture("ix-ci-run-\\{0\\}-\\{1\\}-(?<n>[A-Za-z0-9{}._-]+)") | .n) // null;

def credential_grabs($allow):
  [ .jobs // {}
    | to_entries[]
    | . as $job
    | ($job.value | runner_strings)[]
    | claim_name
    | select(. != null)
    | . as $name
    # `$sfx` bound before the `endswith`, and this is the THIRD instance of the
    # same trap in this file: `$name | endswith(.)` evaluates its argument with
    # `.` rebound to $name, so it asks whether $name ends with itself and every
    # job matched `-cache-push`. Caught only by running it over the real tree
    # and disbelieving a result that flagged 29 of 29 workflows.
    # Against `-<name>`, not `<name>`: the dispatcher matches the whole rendered
    # label `ix-ci-run-<run>-<attempt>-<name>`, so the job called `cache-warm`
    # DOES end in `-cache-warm` and must be caught, while `cache-warm-reaper`
    # does not and must not be.
    | ("-" + $name) as $tail
    | (credential_suffixes | map(. as $sfx | select($tail | endswith($sfx))))[0]
    | select(. != null)
    | . as $suffix
    | select($allow | has("\($workflow | split("/") | last):\($job.key):credential") | not)
    | {job: $job.key, name: $name, suffix: $suffix}
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
| crossrepo_calls_without_label($allow) as $crossrepo
| credential_grabs($allow) as $grabs
| if ($jobs | length) > 0
  # The message teaches the two things an author has to get right after the
  # claim itself, because both fail silently and neither is guessable. Wording
  # follows ix#9423, whose version said this better than the original here did.
  then error("\($workflow): job \($jobs[0].job) does not claim a fleet runner -- \($jobs[0].labels | map(describe(.)) | join("; ")). Every job in this repository runs on an ix-ci-dispatcher runner:\n    runs-on: [\"${{ format('ix-ci-run-{0}-{1}-<name>', github.run_id, github.run_attempt) }}\"]\n  Pick <name> deliberately: the dispatcher grants fleet-trust credentials to labels ENDING in `-cache-push`, `-cache-warm` or `-prod` (crates/ci/dispatcher/src/handler/workflow_job/credential_gate.rs), so extend past them rather than landing on one (`-cache-warm-reaper`, not `-cache-warm`).\n  The fleet PATH carries only bash, coreutils, dash, git, github-runner, tar, gzip, unzip and nix, so provision `gh`, `jq` and friends with a literal `nix build --no-link --print-out-paths nixpkgs#gh`. Spell the installables out: workflow-nix-bootstrap.jq withdraws its nixpkgs-only exemption once an argument contains `$`.\n  If the fleet genuinely cannot serve this job, add it to `allowlist` in packages/workflow-hosted-runner/default.nix with a written reason; do not widen the pattern.")
  elif ($defaults | length) > 0
  then error("\($workflow): the `workflow_call` input `\($defaults[0].input)` defaults to `\($defaults[0].default)`, which is not a fleet claim, and a job here uses that input as its `runs-on`. A default is what every caller that does not override it gets, including callers in other repositories, so a hosted default puts a hosted runner in runs nobody reading this repository would find. Default it to a dispatcher claim, as .github/workflows/update-flake-lock.yml does: `runs-on: [\"${{ inputs.runner-label || format('ix-ci-run-{0}-{1}-<name>', github.run_id, github.run_attempt) }}\"]`.")
  elif ($calls | length) > 0
  then error("\($workflow): job \($calls[0].job) passes `\($calls[0].input): \($calls[0].value)` to a reusable workflow, which is a runner choice made at the call site. Pass a fleet claim or pass nothing.")
  elif ($crossrepo | length) > 0
  then error("\($workflow): job \($crossrepo[0].job) calls `\($crossrepo[0].uses)` in another repository without passing `runner-label`, so it takes whatever that repository defaults to -- a value nothing here can see, and today that default is `ubuntu-latest`. Pass a run-scoped claim explicitly. Local `./` calls need nothing, because their `runs-on` is in this tree and this guard already reads it.")
  elif ($grabs | length) > 0
  then error("\($workflow): job \($grabs[0].job) claims the label `...-\($grabs[0].name)`, which ENDS in `\($grabs[0].suffix)`. The dispatcher grants a fleet-trust credential on exactly that suffix (crates/ci/dispatcher/src/handler/workflow_job/credential_gate.rs): `-cache-push` is the ix-public push token, `-cache-warm` the internal nix-cache SIGNING key, `-prod` the deploy publish channel. A job picks these up by NAME alone, so extend the suffix past it (`-cache-warm-reaper`, not `-cache-warm`) unless this job is genuinely entitled to that credential, in which case name it in the allow list.")
  else true
  end
