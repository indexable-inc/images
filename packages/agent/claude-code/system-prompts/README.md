# Claude Code stock system prompts

These are stock upstream Claude Code `system` snapshots captured through the real
binary with a local `ANTHROPIC_BASE_URL` server. They intentionally exclude the
per-run `x-anthropic-billing-header` block and normalize extraction temp paths so
diffs show prompt changes only.

Refresh them from the pinned Claude Code package:

```sh
nix run .#claude-code.updateScript -- --prompts-only
```

`models.json` is the source of truth for which model aliases are captured.
