---
name: notion
description: "Use Notion from the ix repo shell with the official ntn CLI: authenticate, query or mutate the Notion API, manage Notion Workers, and upload files."
---

Use the official Notion CLI, `ntn`, from the repo direnv shell:

```bash
direnv exec . ntn --version
```

## Authentication

If the task needs a live Notion workspace and `ntn` is not already authenticated, run:

```bash
direnv exec . ntn login
```

This opens a browser authorization flow and stores credentials in the system keychain. If the environment is non-interactive or browser auth is impossible, stop and explain that Notion CLI login must be completed interactively.

## Common Commands

Use `ntn api` for direct authenticated API calls:

```bash
direnv exec . ntn api v1/users
direnv exec . ntn api v1/pages parent[page_id]=PAGE_ID properties[title][title][0][text][content]=Title
direnv exec . ntn api v1/pages/PAGE_ID -X PATCH archived:=true
```

Use `ntn workers` for Notion Workers:

```bash
direnv exec . ntn workers new
direnv exec . ntn workers deploy
direnv exec . ntn workers list
```

Use `ntn files` for static assets:

```bash
direnv exec . ntn files create < image.png
direnv exec . ntn files create --external-url https://example.com/image.png
direnv exec . ntn files list
```

## Guardrails

- Prefer `direnv exec . ntn ...` so the repo-pinned CLI is used.
- Before mutating pages, databases, workers, or files, identify the target workspace/resource from CLI output or explicit user input.
- For destructive changes, state the exact Notion resource and operation before running the command unless the user already gave that exact command.
- Do not place Notion credentials in repo files, `.env`, command history snippets, or PR text.
- For command details, inspect `direnv exec . ntn --help` or the relevant subcommand help.
