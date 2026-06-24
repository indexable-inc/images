# Environment variables

This is a **curated** reference to the environment variables you are most likely
to set when driving `ix`, its SDKs, and the search and MCP tools - not an
exhaustive dump. Many more internal, computed, and wrapper-only variables exist
in the codebase. To see the full set, grep the repos (for example
`rg "env::var|os\.environ|process\.env|env = \"IX_"`). Everything below was
verified by opening the source that reads it; each row cites `path:line`.

## CLI and auth

These are the variables an `ix` user actually exports. The CLI flags shadow them
(`--profile`, `--debug`, `--admin`); the env var is the no-flag default. The
SDKs (TypeScript, Python, and the Rust core they wrap) resolve a token and base
URL the same way. See [cli.md](cli.md).

| var | meaning | default | source (path:line) |
| --- | --- | --- | --- |
| `IX_TOKEN` | API bearer token. Required to talk to the ix platform. | none | ix CLI |
| `IX_API_KEY` | Token fallback in the TS SDK if `IX_TOKEN` is unset. | none | `sdk/typescript/src/index.ts:1421` |
| `IX_API_BASE_URL` | API base URL (TS SDK). | `https://api.ix.dev` | `sdk/typescript/src/index.ts:1428` |
| `IX_REGION` | Pin VMs to a region instead of letting the API pick. | `us-west-1` (Python); first region the API returns (TS) | `sdk/python/ix_sdk/__init__.py:810`, `sdk/typescript/src/index.ts:1437` |
| `IX_PROFILE` | Config profile to use (`--profile`). | none | ix CLI |
| `IX_DEBUG` | Enable CLI debug tracing (`--debug`). Truthy value. | off | ix CLI |
| `IX_ADMIN` | Use admin privileges, bypassing ownership checks (`--admin`). Truthy value. | off | ix CLI |

`IX_TOKEN` is the one most paths require: `ix run` and the SDKs error out
without it.

## Search credentials

The `search` CLI (`nix run .#search`) and the `indexer` authenticate to
Mixedbread. **`MXBAI_API_KEY` is required**: without it (and without a prior
`mgrep login`) `.#search` fails at auth.

| var | meaning | default | source (path:line) |
| --- | --- | --- | --- |
| `MXBAI_API_KEY` | Mixedbread API key. Required unless you ran `mgrep login`. | none | `packages/search/mixedbread/src/lib.rs:40` |
| `MXBAI_STORE` | Store name to query/index (`--store`). | `index` | `packages/search/search/src/main.rs:318`, `packages/search/search-core/src/config.rs:8` |
| `MXBAI_BASE_URL` | Mixedbread API base URL (`--base-url`). | `https://api.mixedbread.com` | `packages/search/search/src/main.rs:322`, `packages/search/mixedbread/src/lib.rs:30` |

## Run recorder

The `run` wrapper records a command's output to a session directory and prints a
summary. These tune that behavior.

| var | meaning | default | source (path:line) |
| --- | --- | --- | --- |
| `IX_RUN_DIR` | Session directory root. | `./.ix/run` | `packages/tui/run/run.py:334` |
| `IX_RUN_PRINT` | Output mode: `summary`, `full`, or `none`. | `summary` | `packages/tui/run/run.py:326` |
| `IX_RUN_HEAD_LINES` | First lines to print in the summary. | `2` | `packages/tui/run/run.py:721` |
| `IX_RUN_TAIL_LINES` | Last lines to print in the summary. | `2` | `packages/tui/run/run.py:722` |

## ix-mcp

User-tunable variables for the notebook MCP server. (Internal and
fleet-injected ones such as `IX_MCP_EXEC_TOKEN` and `IX_MCP_SHARED` are omitted.)

| var | meaning | default | source (path:line) |
| --- | --- | --- | --- |
| `IX_MCP_MAX_RESULT_CHARS` | Max characters of a tool result before paging kicks in. Floor 500. | `50000` | `packages/mcp/ix_notebook_mcp/outputs.py:25` |
| `IX_MCP_SESSION` | Checkpoint the session to a file; set by `serve --session FILE`. | unset (off) | `packages/mcp/ix_notebook_mcp/runtime.py:2484` |

## Health-check context (read-only)

These are **injected by the platform into host health checks, not set by you.**
`ix-fleet` populates them per node before running a check on the operator's
machine; the check script reads them to learn about the node under test. Setting
them yourself has no effect on the fleet. See
[health-checks.md](health-checks.md).

| var | meaning | source (path:line) |
| --- | --- | --- |
| `IX_NODE` | Fleet node name. Always set. | `packages/ix-fleet/src/ix_fleet/__init__.py:564` |
| `IX_NODE_NAME` | Branch name reported by the API. | `packages/ix-fleet/src/ix_fleet/__init__.py:566` |
| `IX_NODE_IMAGE` | Image the node is running. | `packages/ix-fleet/src/ix_fleet/__init__.py:567` |
| `IX_NODE_STATUS` | Node status string. | `packages/ix-fleet/src/ix_fleet/__init__.py:568` |
| `IX_NODE_IPV6` | Node IPv6 address. | `packages/ix-fleet/src/ix_fleet/__init__.py:569` |
| `IX_NODE_IPV4` | Node IPv4 address, when assigned. | `packages/ix-fleet/src/ix_fleet/__init__.py:571` |
| `IX_NODE_SUBDOMAIN` | Node subdomain, when assigned. | `packages/ix-fleet/src/ix_fleet/__init__.py:573` |
| `IX_NODE_REGION` | Node region slug, when known. | `packages/ix-fleet/src/ix_fleet/__init__.py:575` |

## See also

- [overview.md](overview.md): where `ix` and these variables sit in the platform.
- [cli.md](cli.md): the `ix` verbs the CLI/auth variables shadow.
- [health-checks.md](health-checks.md): the host checks that read the `IX_NODE_*` variables.
