# mcp server

The server is the host process: the `ix-mcp` CLI, the one kernel it manages, the
MCP transport, the three MCP tools, and the generated instructions a client reads
at `initialize`. It does not run user code itself: it forwards each call into the
single ipykernel where the [runtime](../runtime/overview.md) executes it, then
renders the kernel's reply back over MCP. The in-process plumbing is a frozen
`Config` (`config.py`) decided once by the CLI and read by every other module.

## Entrypoint and CLI (`cli.py`)

`python -m ix_notebook_mcp` runs `cli.main` (`__main__.py:5`, `cli.py:45`). The
Nix wrapper adds `-m ix_notebook_mcp` for the `ix-mcp` binary and
`-m ix_notebook_mcp notebook` for the `ix-notebook` binary (see [build](#build)).
Subcommands (`cli.py:49-81`):

| command | what it does |
| --- | --- |
| `serve` | run the MCP server (default command, `cli.py:84`). `--http [ADDR]` serves streamable HTTP instead of stdio; `--session FILE` makes the store a persistent notebook; `--workdir DIR` sets the kernel cwd. |
| `notebook [FILE]` | the engine alone (kernel + dashboard + optional session file, no MCP transport); `transport="none"` (`cli.py:294`). The same engine the MCP server is one client of. |
| `dashboard` | open the one shared dashboard UI, starting it if needed (`_dashboard`). Reuses a running hub recorded in `runtime_dir()/hub.json` (`config.live_hub`, port-probed and pid-checked) or spawns one detached on a stable port (8080, `IX_DASH_HUB_PORT`); `--no-open` prints the URL without opening a browser. |
| `requirements` | report each external credential as present (and from where) or missing (and the remedy); exits non-zero if any is missing (`cli.py:89`). See [requirements](#credentials-requirements). |
| `eval EXPR` / `exec SRC` | run one expression/statements on a throwaway kernel via `jupyter_client.start_new_kernel` (`cli.py:557`); not the shared kernel. |

`_serve` (`cli.py:286`) resolves everything before the kernel spawns: the
dashboard bind host (`IX_MCP_HOST`, else the Tailscale IPv4 when up, else
loopback, probed for bindability and degraded to `127.0.0.1` if unbindable,
`cli.py:321-334`), the data-API port (`IX_MCP_DASHBOARD_PORT` or a free port,
`cli.py:145`), the Loro hub port (`cli.py:382`), the store path, and the session
mode. It builds the frozen `Config`, sets `IX_MCP_STORE`/`IX_MCP_DASHBOARD_URL`/
`IPYTHONDIR` in the environment the kernel inherits (`cli.py:406-411`), then
`asyncio.run(_run(cfg))`.

`_run` (`cli.py:471`) is the lifecycle: start the kernel, set it as the global
kernel, optionally restore a session (holding the shell channel until the restore
locks it so later calls queue behind it, `cli.py:480-499`), start the dashboard
data API, spawn the Loro hub and the pane bridge, bake the live URL into the
instructions (`tools.set_dashboard_url`), then `transport.serve()`. On exit it
checkpoints the session, cleans up the runner, and shuts the kernel down.

## Transports (`transport.py`)

stdio is what MCP clients launch and the default. The CLI dups the real
stdin/stdout to private fds and points fd 0/1 at `/dev/null`+stderr first
(`cli.py:299-306`), so nothing else can corrupt the JSON-RPC stream;
`_serve_stdio` hands the low-level `mcp` server those private fds
(`transport.py:25`). `--http` selects `mcp.run_streamable_http_async()` bound to
`mcp_http_host:port` (default `127.0.0.1:8000`, `transport.py:39`). The transport
serves the FastMCP server `mcp` from `tools.py`.

## Kernel manager (`kernel.py`)

`Kernel` owns the one `AsyncKernelManager(kernel_name="python3")` and its client
(`kernel.py:78`). `python_exec` wraps the user code as
`await __ix_exec(<code repr>, budget=..., name=..., session=...)` and runs it with
`timeout = budget + wedge_grace` (`kernel.py:204-211`). `_execute`
(`kernel.py:132`) holds one `asyncio.Lock`, shields the `execute_interactive`
task from client-side cancellation (a cancel mid-reply would desync the shared
shell socket and hang every later call, `kernel.py:154-183`), and collects the
IOPub outputs plus the runtime's structured job summary (`outputs.job_summary`).

Two rescue paths for a wedged kernel (a cell blocking the event loop with a
synchronous call):

- `kernel_trace` -> `dump_trace` (`kernel.py:103`) sends `SIGUSR1` to the kernel
  pid; the runtime's faulthandler writes an all-thread stack to the
  `IX_MCP_KERNEL_TRACE` file even while the loop is frozen, and the newest dump is
  returned.
- A `TimeoutError` past `budget + wedge_grace` -> `_interrupt` (`kernel.py:216`)
  sends `SIGUSR2`, which the runtime handler turns into an inline
  `KeyboardInterrupt` at the blocked frame (a plain task-cancel cannot break a
  synchronous call), and the call returns an actionable `_wedged_summary`
  (`kernel.py:33`).

`restore_session`/`snapshot_session` (`kernel.py:233`, `kernel.py:242`) drive the
session machinery; see [sessions](../sessions/overview.md).

## MCP tools (`tools.py`)

The FastMCP server is `mcp = FastMCP("ix-mcp")` (`tools.py:75`); its
`serverInfo.version` is stamped from `IX_MCP_VERSION` (the flake rev) onto the
low-level server (`tools.py:197`). Three tools, all with
`structured_output=False` so FastMCP does not duplicate the reply as
`structuredContent` (which doubled image blocks and blew the token cap,
`tools.py:202-207`):

- `python_exec(code, budget=15.0, intent=None)` (`tools.py:219`): clamps `budget`
  to `config().max_budget`, calls `current_kernel().python_exec(...)`, renders the
  kernel outputs with `outputs.to_mcp`, and appends notices when the budget was
  clamped or the reply was truncated (pointing the caller at `jobs['<id>']` paging
  helpers). `intent` is the run's human-facing title in the dashboard feed.
- `read(target, start=None, end=None)` (`tools.py:289`): pulls a file or a kernel
  value into the model's context via `await __ix_read(...)` while the dashboard
  shows only a one-line note, so a large file does not flood the human's view.
- `kernel_trace()` (`tools.py:315`): the out-of-band stack dump for a wedged
  kernel.

The first tool call of a session pops the dashboard in the human's browser once
(`_open_dashboard_once`, `tools.py:162`; disabled by `IX_MCP_NO_BROWSER=1`).

### Generated instructions

`_compose_instructions` (`tools.py:133`) builds the server `instructions` from
the authored `guide.py` fragments (`_KERNEL_GUIDE`, `tools.py:54`), a per-tool
overview derived from the registry (`_tools_overview`, `tools.py:123`), and the
live dashboard URL once it is bound (`set_dashboard_url`, `tools.py:144`). The
bundled-module index and the credentialed-services sentence come from
`registry.py` via `guide.modules_index()`/`guide.credentials_note()`, so every
tool and module is described in exactly one place. Fragment ORDER is deliberate:
clients truncate long instruction blocks from the tail, so the rules that shape
every call come first and operational mechanics follow (`tools.py:47-53`).

## Configuration (`config.py`)

`Config` is a frozen dataclass set once via `set_config` and read with `config()`
(`config.py:104-115`). Key fields: `workdir`, `host`/`advertised_host`,
`dashboard_port`/`hub_port`, `store_path`/`session_path`/`session_resume`,
`transport`, `mcp_http_host`/`port`, `stdin_fd`/`stdout_fd`, `exec_token`/
`exec_trust_network` (gating `/api/exec`), `wedge_grace=15.0`, `max_budget=120.0`.
`resolve` rejects a path that escapes `workdir` (`config.py:96`). `runtime_dir()`
(`config.py:118`) is a hardened 0700 dir under `XDG_RUNTIME_DIR`/`TMPDIR`/`/tmp`
holding the store and the `dashboard-url` handoff (CWE-377 checks).

Environment variables the server reads: `IX_MCP_HOST`, `IX_MCP_PUBLIC_HOST`,
`IX_MCP_DASHBOARD_PORT`, `IX_MCP_HUB_PORT`, `IX_MCP_STORE`, `IX_MCP_SESSION`,
`IX_MCP_NO_BROWSER`, `IX_MCP_EXEC_TOKEN`(`_FILE`), `IX_MCP_EXEC_TRUST_NETWORK`,
`IX_MCP_VERSION`, `IX_MCP_DASHBOARD_URL`, `IX_MCP_KERNEL_TRACE`, plus output caps
read in [runtime](../runtime/overview.md) and `outputs.py`
(`IX_MCP_MAX_RESULT_CHARS`, `IX_MCP_IMAGE_MAX_BYTES`, `IX_MCP_IMAGE_MAX_DIM`).

## Output rendering (`outputs.py`)

`output_from_message` turns IOPub messages into nbformat output dicts
(`outputs.py:56`); `to_mcp` renders them as MCP content blocks for the agent
(`outputs.py:71`): real image blocks for plots, a `Result`'s explicit model view
from the `application/x-ix-llm+json` bundle, and text clipped to `MAX_TEXT_CHARS`
(default 50k, `outputs.py:26`) as a head+tail preview that points at paging. The
internal job summary (`application/x-ix-job+json`, `outputs.py:47`) is pulled out
separately by `job_summary` and never shown as content. Oversize images are
dropped with a note rather than dumped as base64 (`outputs.py:131`).

## Credentials (`requirements.py`)

`statuses()` probes every credential declared in `registry.credentialed()`
locally (env vars and token-file existence only, never the value or the network,
`requirements.py:44`). `report(emit)` prints one line each and returns whether
all are present (`requirements.py:82`). Three fail-fast consumers: `ix-mcp serve`
yells on stderr at startup (`cli.py:430`), the `requirements` subcommand turns the
bool into its exit code, and the instructions name the credentialed modules.

## Build (`default.nix`)

`default.nix` builds the pinned interpreter with `pkgs.python3.withPackages`
(`default.nix:893`), bundling the data libraries (numpy, polars, duckdb, httpx,
matplotlib, pypdf), the execution engine (`ipykernel`, `jupyter-client`,
`nbformat`, `aiohttp`, the `mcp` SDK, `dill`, `ray`, a Spark Connect `pyspark`,
`playwright`, `htpy`, `ansi2html`), and every first-party module as a
`toPythonModule` (`ix_notebook_mcp` plus each `src/<module>`, and cdylib modules
`tui`/`search`/`astlog`/`scipql`/`flecs_query`/`fff`/`ix_google` bundled from
other packages). `makeWrapper` then emits two binaries (`default.nix:918-948`):

- `ix-mcp` (`mainProgram`): `<interpreter> -m ix_notebook_mcp`, with
  `IX_MCP_VERSION` (flake rev), `PLAYWRIGHT_BROWSERS_PATH`, `IX_GCAL_BIN`
  (the `gcal` binary), `IX_DASHBOARD_BIN` (the `dashboard` hub binary),
  `SCIPQL_SOUFFLE`, and on Darwin `IX_VMKIT_BIN`.
- `ix-notebook`: the same interpreter entered at the `notebook` subcommand.

Flake output: `nix run .#mcp -- serve` / `nix build .#mcp` (`package.nix:2-4`).
`passthruTests = true` (`package.nix:5`) wires the in-`default.nix` import and
behavior smoke tests (e.g. `serverTools`, `runtimeSmoke`, `sessionSmoke`,
`feedSmoke`, per-module `*Bundled`/`*Smoke`) as passthru checks
(`default.nix:4220-4275`).
