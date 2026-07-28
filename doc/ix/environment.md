# Environment variables

This is a **curated** reference to the environment variables you are most likely
to set when driving `ix`, its SDKs, and the search and MCP tools - not an
exhaustive dump. Many more internal, computed, and wrapper-only variables exist
in the codebase. To see the full set, grep the repos (for example
`rg "env::var|os\.environ|process\.env|env = \"IX_"`). Everything below was
verified by opening the source that reads it; each row cites the repo-relative path.

## CLI and auth

These are the variables an `ix` user actually exports. The CLI flags shadow them
(`--profile`, `--debug`, `--admin`); the env var is the no-flag default. The
SDKs (TypeScript, Python, and the Rust core they wrap) resolve a token and base
URL the same way. See [cli.md](cli.md).

| var | meaning | default | source path |
| --- | --- | --- | --- |
| `IX_TOKEN` | API bearer token. Required to talk to the ix platform. | none | ix CLI and TS SDK (ix monorepo, `crates/ix/sdk-ts`) |
| `IX_API_KEY` | Token fallback in the TS SDK if `IX_TOKEN` is unset. | none | TS SDK (ix monorepo, `crates/ix/sdk-ts`) |
| `IX_API_BASE_URL` | API base URL (TS SDK). | `https://api.ix.dev` | TS SDK (ix monorepo, `crates/ix/sdk-ts`) |
| `IX_REGION` | Pin VMs to a region instead of letting the API pick. | `us-west-1` (Python); first region the API returns (TS) | Python and TS SDKs (ix monorepo, `crates/ix/sdk-py`, `crates/ix/sdk-ts`) |
| `IX_PROFILE` | Config profile to use (`--profile`). | none | ix CLI (ix monorepo) |
| `IX_DEBUG` | Enable CLI debug tracing (`--debug`). Truthy value. | off | ix CLI (ix monorepo) |
| `IX_ADMIN` | Use admin privileges, bypassing ownership checks (`--admin`). Truthy value. | off | ix CLI (ix monorepo) |

`IX_TOKEN` is the one most paths require: `ix run` and the SDKs error out
without it.

## Search credentials

The `search` CLI (`nix run .#search`) and the `indexer` authenticate to
Mixedbread. **`MXBAI_API_KEY` is required**: without it (and without a prior
`mgrep login`) `.#search` fails at auth.

| var | meaning | default | source path |
| --- | --- | --- | --- |
| `MXBAI_API_KEY` | Mixedbread API key. Required unless you ran `mgrep login`. | none | `packages/mixedbread/src/lib.rs` |
| `MXBAI_STORE` | Store name to query/index (`--store`). | `index` | `packages/search/src/main.rs` |
| `MXBAI_BASE_URL` | Mixedbread API base URL (`--base-url`). | `https://api.mixedbread.com` | `packages/search/src/main.rs` |

## Run recorder

The `run` wrapper records a command's output to a session directory and prints a
summary. These tune that behavior.

| var | meaning | default | source path |
| --- | --- | --- | --- |
| `IX_RUN_DIR` | Session directory root. | `./.ix/run` | `packages/tui/run/run.py` |
| `IX_RUN_PRINT` | Output mode: `summary`, `full`, or `none`. | `summary` | `packages/tui/run/run.py` |
| `IX_RUN_HEAD_LINES` | First lines to print in the summary. | `2` | `packages/tui/run/run.py` |
| `IX_RUN_TAIL_LINES` | Last lines to print in the summary. | `2` | `packages/tui/run/run.py` |

## Health-check context (read-only)

These are **injected by the platform into host health checks, not set by you.**
`ix-fleet` populates them per node before running a check on the operator's
machine; the check script reads them to learn about the node under test. Setting
them yourself has no effect on the fleet. See
[health-checks.md](health-checks.md).

| var | meaning | source path |
| --- | --- | --- |
| `IX_NODE` | Fleet node name. Always set. | `packages/ix-fleet/src/ix_fleet/__init__.py` |
| `IX_NODE_NAME` | Branch name reported by the API. | `packages/ix-fleet/src/ix_fleet/__init__.py` |
| `IX_NODE_IMAGE` | Image the node is running. | `packages/ix-fleet/src/ix_fleet/__init__.py` |
| `IX_NODE_STATUS` | Node status string. | `packages/ix-fleet/src/ix_fleet/__init__.py` |
| `IX_NODE_IPV6` | Node IPv6 address. | `packages/ix-fleet/src/ix_fleet/__init__.py` |
| `IX_NODE_IPV4` | Node IPv4 address, when assigned. | `packages/ix-fleet/src/ix_fleet/__init__.py` |
| `IX_NODE_SUBDOMAIN` | Node subdomain, when assigned. | `packages/ix-fleet/src/ix_fleet/__init__.py` |
| `IX_NODE_REGION` | Node region slug, when known. | `packages/ix-fleet/src/ix_fleet/__init__.py` |

## See also

- [overview.md](overview.md): where `ix` and these variables sit in the platform.
- [cli.md](cli.md): the `ix` verbs the CLI/auth variables shadow.
- [health-checks.md](health-checks.md): the host checks that read the `IX_NODE_*` variables.
