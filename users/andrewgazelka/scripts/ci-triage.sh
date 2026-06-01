# Body of the `ci-triage` bash app (see users/andrewgazelka/home.nix).
# No shebang / `set` line: the mkBashApp wrapper supplies bash + `set -euo
# pipefail` and bakes gh/jq/claude/coreutils onto PATH.
#
# Stage-2 of the pr-watch CI response: a per-run DEEP DIVE into one GitHub
# Actions run that just FAILED on main. pr-watch (stage 1) already showed the
# failure as a silent angry-villager pop in the feed overlay; this script is
# launched DETACHED (own session, see the ci-triage invocation in pr-watch) so it
# never blocks stage 1 for the next failure and survives a launchd/systemd reload,
# and it is wrapped in `timeout` so a stuck Opus run can't pile up.
#
# It uses `claude -p` with Opus 4.8 and the Bash tool (it needs tools to read
# the failed logs and file a ticket). It is SILENT: the villager pop is the only
# alert, so this stage neither plays a sound nor speaks. The agent:
#   1. Fetches failed logs itself (gh run view --log-failed, capped).
#   2. Diagnoses the ROOT CAUSE.
#   3. Decides if it's a genuine, actionable code failure vs transient/infra/flaky.
#   4. If actionable: searches Linear for an existing matching open issue first
#      (dedupe), and only creates a new ENG ticket via issueCreate if none.
#
# Linear auth: the key is read at runtime from $PR_WATCH_LINEAR_KEY, else the
# macOS login Keychain (service `pr-watch-linear`) — no secret in the Nix store,
# the plist, or git. Seed the Keychain (idempotent, from 1Password) with the
# host's `seed-launchd-secrets`. If neither is present the deep dive still runs;
# it just notes in its summary that it cannot file a ticket.
#
# CI_TRIAGE_DRY_RUN=1 makes this NON-DESTRUCTIVE for testing: NO real Linear
# ticket — the agent prints the GraphQL mutation it WOULD send instead. Logs are
# still fetched and Opus still analyses.
#
# Usage: ci-triage <repo> <runId> <url> <workflow> <jobs>
#   e.g. ci-triage indexable-inc/ix 123 https://... ci "build (step: clippy)"

repo="${1:?ci-triage: missing <repo>}"
run_id="${2:?ci-triage: missing <runId>}"
run_url="${3:?ci-triage: missing <url>}"
workflow="${4:?ci-triage: missing <workflow>}"
jobs="${5:-(unknown jobs)}"
short="${repo##*/}"

dry="${CI_TRIAGE_DRY_RUN:-}"

# Linear API key (best-effort: tickets are optional). Resolution order, mirroring
# ix-downtime's token lookup:
#   1. $PR_WATCH_LINEAR_KEY, if the consuming config exports one (e.g. a Linux
#      host that seeds it from its own secret store).
#   2. the macOS login Keychain (service `pr-watch-linear`), seeded by the host's
#      `seed-launchd-secrets`. `security` is macOS-only, so guard on its presence.
linear_key="${PR_WATCH_LINEAR_KEY:-}"
if [ -z "$linear_key" ] && command -v security >/dev/null 2>&1; then
  linear_key="$(security find-generic-password -s pr-watch-linear -w 2>/dev/null)" || linear_key=""
fi

# ENG team id (from the repo's linear skill). Linear auth header is the raw key
# (NOT "Bearer ...").
eng_team_id="a8845362-21c7-4283-ba80-cea987a3ee74"

# Compose the agent's task. Everything the agent needs is inlined; it fetches
# logs and (maybe) files the ticket via its Bash tool.
ticket_clause=""
if [ -n "$dry" ]; then
  ticket_clause="DRY RUN MODE: Do NOT create a real Linear ticket. Instead, after your analysis, PRINT to stdout: your applicable/not-applicable decision with one-line reason (prefixed \"DECISION: \"), and if applicable the exact Linear GraphQL issueCreate mutation JSON you WOULD POST (prefixed \"WOULD POST: \"). Do not run any curl/POST to Linear."
elif [ -n "$linear_key" ]; then
  ticket_clause="You CAN file a Linear ticket. The Linear API key is in the login Keychain; read it in Bash with: LINEAR_API_KEY=\$(security find-generic-password -s pr-watch-linear -w). The Linear GraphQL endpoint is https://api.linear.app/graphql (POST, Content-Type: application/json, Authorization header is the RAW key, NOT Bearer)."
else
  ticket_clause="You CANNOT file a Linear ticket right now (no key in the Keychain). Do your analysis and note in your summary that you could not open a ticket because the Linear key is missing."
fi

read -r -d '' task <<EOF || true
A GitHub Actions run just FAILED on the main branch of $repo. Triage it end to end.

Facts:
- Repo: $repo (say this short name: $short)
- Run id: $run_id
- Run URL: $run_url
- Workflow: $workflow
- Failed job(s)/step(s): $jobs

Do the following, in order, using your Bash tool:

1. Fetch the failed logs yourself:
     gh run view $run_id --repo $repo --log-failed 2>/dev/null | tail -n 400 | head -c 16000
   (Cap the output as shown so it stays bounded. If empty, note that and rely on the job/step names.)

2. Diagnose the ROOT CAUSE in detail from the logs.

3. Decide if this is a GENUINE, ACTIONABLE code failure: a real test, build, or lint break caused by a change. If it is clearly transient/infra/flaky (runner lost, network error, cache miss, timeout with no code signal, cancelled), it is NOT actionable: do not file a ticket, and note that briefly in your summary.

4. If ACTIONABLE: FIRST search Linear for an existing OPEN issue matching this failure (same workflow+job signature, or the same run url in the description) to avoid duplicates. Use the searchIssues query with the "term" argument (NOT "query"): searchIssues(term: "main CI failed: $workflow", first: 10) { nodes { identifier title url } } and inspect titles/descriptions. Only if NONE matches, create one with issueCreate:
     - teamId: "$eng_team_id" (the ENG / Engineering team)
     - title: "main CI failed: $workflow/<failing job> ($repo)"
     - description (markdown): the run URL ($run_url), the failing job/step ($jobs), and your full root-cause analysis. End the description with this exact footer on its own line:
         _Filed automatically by pr-watch via Claude Opus 4.8._
   After creating (or matching), include the ticket identifier in your summary (e.g. "Filed ENG-1234" or "Matched existing ENG-1234").

$ticket_clause

Output a short plain-text summary of what you did (root cause, decision, ticket id or none). No preamble.
EOF

echo "CI-TRIAGE: [$short run $run_id] workflow=$workflow dry=${dry:-0}"

# Isolated, headless deep dive: Opus 4.8 with the Bash tool. Bounded by timeout
# in the caller too, but cap here as well so a direct invocation is also safe.
cd "$HOME" || exit 1
timeout 280 claude -p \
  --model claude-opus-4-8 \
  --allowedTools "Bash" \
  --setting-sources "" \
  "$task" </dev/null 2>&1 || echo "CI-TRIAGE: claude exited non-zero (timeout or error) for run $run_id"
