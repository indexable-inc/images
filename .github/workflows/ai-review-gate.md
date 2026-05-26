# AI Review Gate

`ai-review-gate.yml` runs a structured pull request review and publishes GitHub
review suggestions for actionable findings. The workflow is model-neutral at
the repo boundary: callers choose the status check name and model input, while
the current implementation uses OpenAI's reviewer action internally with
`xhigh` reasoning effort by default.

Same-repository PRs opened by normal users run the secret-backed reviewer. Fork
and Dependabot PRs stay on a no-secret path and pass only after a trusted
maintainer approves the current head commit.

## Reuse From Another Repo

Create a small caller workflow in the consuming repository:

```yaml
name: AI review gate

on:
  pull_request:
    branches:
      - main
    types:
      - opened
      - synchronize
      - reopened
      - ready_for_review
  pull_request_target:
    branches:
      - main
    types:
      - opened
      - synchronize
      - reopened
      - ready_for_review
  pull_request_review:
    types:
      - submitted
      - edited
      - dismissed

permissions:
  contents: read

jobs:
  ai-review-gate:
    permissions:
      actions: read
      checks: read
      contents: read
      issues: write
      pull-requests: write
    uses: indexable-inc/index/.github/workflows/ai-review-gate.yml@main
    with:
      caller_event_name: ${{ github.event_name }}
      required_check_name: ai review approved
    secrets:
      openai_api_key: ${{ secrets.OPENAI_API_KEY }}
      repository_token: ${{ secrets.GITHUB_TOKEN }}
```

Set `AI_REVIEW_MODEL` as a repository variable when a repo should use a model
other than the workflow default. Set `AI_REVIEW_EFFORT` or pass the `effort`
input when a repo needs a value other than `xhigh`.

For `workflow_call` consumers, branch protection should require the check name
GitHub reports for the called job, which includes the caller job and the final
gate job. With the example above, require
`ai-review-gate / ai review approved`. Direct users of this workflow can set
`AI_REVIEW_REQUIRED_CHECK_NAME` during a status-check migration when an existing
ruleset already requires another gate name.
