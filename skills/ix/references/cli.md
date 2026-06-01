# CLI

```bash
curl -fsSL https://ix.dev/install.sh | sh
```

Auth reads `IX_TOKEN` from the environment. Create one at https://ix.dev/tokens.

Run `ix --help` for the full command list. Every subcommand supports `--help`. From inside the [index](https://github.com/indexable-inc/index) monorepo you can run the same pinned binary without installing it: `nix run .#ix -- --help`. Treat that output as the source of truth for command syntax; this page can lag the CLI.

> [!NOTE]
> Thin wrapper over the API. Everything here is also available via [Python](sdk/python.md) and [TypeScript](sdk/typescript.md) SDKs.
