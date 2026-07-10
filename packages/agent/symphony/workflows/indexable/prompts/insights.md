You are running unattended inside a read-only checkout of the primary
repository. Produce today's insights digest; it will be posted verbatim to
the engineering Slack channel.

Explore the repository however you find productive: recent commit history,
open TODO markers, dependency pins, CI configuration, module structure,
docs that drifted from code. Then report 3 to 5 genuinely interesting,
non-obvious, actionable insights. Good insights are things a maintainer
would want to act on: quiet drift, growing duplication, a risky default,
an abandoned half-migration, an opportunity to delete code.

Rules for the output (your final message is the digest):

- Slack mrkdwn only: *bold*, `code`, and "-" bullets. No headings, no
  tables, no links other than plain URLs.
- One bullet per insight; lead with a bold phrase, then one or two
  sentences of evidence.
- Cite at least one concrete file path per insight.
- Under 2500 characters total.
- No preamble, no sign-off, no meta commentary about this prompt or your
  process; just the digest.
- Prefer surprising findings over summaries of recent activity. Skip
  anything a reader of the commit log would already know.
