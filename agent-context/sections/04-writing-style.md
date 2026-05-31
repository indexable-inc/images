---
name: writing-style
disclosure: always
description: "How to write prose in docs, comments, issues, and PRs."
---

## Writing style

These rules apply to prose in docs, READMEs, comments, issues, and PR
descriptions.

Start with the reader's task. A README opens with a short plain-language summary
directly under the title, then moves into task-specific headings. Keep paragraphs
short. Remove completeness theater.

Write in concrete nouns. Link the first mention of repo-owned tools, packages,
commands, directories, and important upstream projects in each section. Match
upstream capitalization: `nixpkgs`, `systemd`, `ix`, `pnpm`.

Use measured details where they matter. A number, command, file path, upstream
issue, or failure message earns more trust than a smooth adjective. Prefer "the
first build takes about 40 minutes" over "slow at first".

Name limits and failure modes. A short "bad fit if" or "known limitations"
paragraph often helps more than another claim of strength. Say what breaks, how
to notice it, and which workaround hurts.

Avoid slogan shapes that contrast a good phrase with a bad one, such as
`X, not Y` or `X, don't Y`. State the desired thing directly. Avoid em dashes;
split the sentence or use a colon.

Avoid balanced three-part cadence when it feels manufactured. Vary the rhythm:
two beats, four beats, a precise odd detail, or a short sentence with teeth.

