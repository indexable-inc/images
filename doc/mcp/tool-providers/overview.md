# mcp tool providers

The providers under `packages/mcp/src` are the bundled Python modules an agent
imports inside a `python_exec` cell to get a capability: search, shell, browser,
fleet, calendar, git worktrees, and more. There is no MCP tool per provider; the
[one general tool](../server/overview.md) plus the import-in-a-cell model means a
provider is reached the same way any Python is. Each provider is its own package
baked into the pinned interpreter by [`default.nix`](../server/overview.md#build),
so it imports with no `pip`/`uv install` step.

## Architecture

- One registry, generated surface. A first-party provider is declared once in
  [`registry.py`](../runtime/overview.md#the-bundled-module-registry-registrypy)
  with a tagline and optional credential. That single row feeds the live `api()`
  catalog (signatures come from introspection, never copied prose), the
  startup pre-import list (`fff`, `view`), and the server instructions
  (`guide.modules_index()`). Adding a row lists the module everywhere with no
  drift.
- Two return audiences. Providers follow the kernel's `Result` contract: a human
  watching the dashboard gets a rich render (a styled polars table, a
  syntax-highlighted file, ANSI color as HTML) while the model gets concise text.
  `sh` returns an `Output` that IS a `Result`; `view`/`fff`/`nix`/`fleet` return
  polars frames the global `_repr_html_` renders. See
  [runtime](../runtime/overview.md#result-runtimepy442).
- Async where it touches the network or a subprocess. The kernel is one shared
  event loop, so anything that would block (`ssh`, HTTP, a child process) is
  `async def` and awaited, or runs off-loop via `asyncio.to_thread`. `sh`,
  `fleet`, `slack`, `linear`, `x`, `browser`, `nix`, `worktree`, `mcp_client` are
  async; `fff`/`view` are sync-but-fast (a cached, file-watching index).
- Credentials are declarative and fail-fast. A credentialed provider's env/token
  sources are declared in `registry.py` and probed by
  [`requirements.py`](../server/overview.md#credentials-requirements); a call with
  a missing credential fails immediately with the remedy.
- Incognito gate. Providers that reach a personal account (`google_auth`,
  `slack`, `x`) refuse to run in a shared (multiplayer) room: the room marks its
  shared MCP with `IX_MCP_SHARED=1` and the provider raises rather than leak
  personal data into a shared transcript.

## Providers (`packages/mcp/src`)

| provider | what it does | key entrypoints | notes |
| --- | --- | --- | --- |
| [`fff`](../../../packages/mcp/src/fff/fff/__init__.py) | typo-tolerant fuzzy file find + SIMD content grep over an in-memory index, bound to the `fff-c` cdylib via ctypes | `find`, `grep`, `tree`, `map` (+ async `afind`/`agrep`/`atree`/`amap`), `FileFinder` | pre-imported; results are dataclasses with a `.df` polars frame; cdylib from `packages/fff` |
| [`view`](../../../packages/mcp/src/view/view/__init__.py) | pretty composable file/listing/search views: directories as polars frames, files as syntax-highlighted renders | `ls`, `tree`, `cat`/`read`, `head`, `tail`, `json`, `diff`, `edit`, `img` | pre-imported; installs the global `DataFrame._repr_html_` (`df_html`) every frame renders through |
| [`sh`](../../../packages/mcp/src/sh/sh/__init__.py) | run a shell command on the async loop, rendered two ways | `await sh(cmd)` -> `Output` (a `Result`); `.text`/`.code` (alias `.exit_code`)/`.ok`, `.json()`/`.jsonl()`/`.df()` | a builtin (pre-bound, no import); ANSI color captured to HTML for the human, stripped for the model; process-group kill on timeout; a non-zero exit renders loudly (leading `[exit N]` failure line, trailing `[exit N]` marker, falsy Output, failure line echoed to the job stream); rendered command text is secret-redacted (`[redacted:<kind>]`) |
| [`nix`](../../../packages/mcp/src/nix/nix/__init__.py) | parse a `nix --log-format internal-json` stream into polars and a live build DAG | `run`, `build`, `attrs`, `eval`, `parse`; `NixLog.events`/`.activities` | pure Python over polars; publishes a live build-DAG dashboard resource |
| [`fleet`](../../../packages/mcp/src/fleet/fleet/__init__.py) | the tailnet as one cluster: SSH fan-out, Ray, Spark Connect, and a peer's live kernel | `scan`/`read_text`/`read_ndjson`; `nodes`/`run`/`submit`/`get`/`put`/`in_kernel`/`spark` (`cluster.py`) | all async; drives Ray + Spark Connect; `in_kernel` calls a peer's `/api/exec` |
| [`browser`](../../../packages/mcp/src/browser/browser/__init__.py) | drive a running browser over CDP with Playwright (debug port 9222 by default) | `get_or_create_browser`, `goto`, `shot`, `read`, `vdom`, `page`, `connect`, `close` | launches a VISIBLE, never-headless window; publishes a live page resource; `vdom()` is a filtered virtual DOM map |
| [`x`](../../../packages/mcp/src/x/x/__init__.py) | read recent X (Twitter) posts into polars by driving the signed-in browser over CDP | `await x.posts(source)` (home, `@handle`, `#tag`, notifications, a thread URL) | reuses `browser`'s CDP connection; incognito only |
| [`worktree`](../../../packages/mcp/src/worktree/worktree/__init__.py) | git worktrees as the unit of isolated/parallel work | `add`, `remove`, `prune`, `list`; `Worktree.build`/`sh`/`commit`, `wt / "path"` | async git; `build` runs `git add -A` first so new files are seen by `nix build` |
| [`mcp_client`](../../../packages/mcp/src/mcp_client/mcp_client/__init__.py) | call any other MCP server's tools from a cell | `await connect(url_or_command)` -> `Server`; `srv.tools` (frame), `await srv.call(tool, **args)`, `close` | thin wrapper over the `mcp` SDK; stdio + streamable-HTTP/SSE; bearer/header auth + interactive OAuth 2.0 PKCE with cached tokens (`_oauth.py`) |
| [`google_auth`](../../../packages/mcp/src/google_auth/google_auth/__init__.py) | Gmail + Calendar for the user's own account via the official googleapiclient | `await login()`, `gmail()`, `calendar()`, `credentials()`, `status()`, `logout()` | credential: Google (token `~/.config/google/token.json` minted by the `gcal` binary, `IX_GCAL_BIN`); incognito only |
| [`slack`](../../../packages/mcp/src/slack/slack/__init__.py) | read Slack channels/DMs/threads to polars, send, search | `channels`, `dms`, `messages`, `thread`, `send`, `search`; `login`/`status`/`logout` | credential: Slack (`SLACK_USER_TOKEN`/`SLACK_TOKEN` or `~/.config/slack/token`); incognito only |
| [`linear`](../../../packages/mcp/src/linear/linear/__init__.py) | Linear issue tracker over GraphQL | `issue`, `issue_update`, `issue_create`, `project_create`, `issue_search`, `comment_create` | credential: Linear (`LINEAR_API_KEY`); `triage.py` is a source-agnostic dedup/triage core (`Finding`, `TriageConfig`, `triage`) |
| [`nox_autotriage`](../../../packages/mcp/src/nox_autotriage/nox_autotriage/__init__.py) | convert a nox conformance JSON report into deduped Linear issues via `linear.triage` | `findings_from_conformance`, `config_from_env`, `run`; `python -m nox_autotriage --report PATH` | a CI/Symphony CLI, not a kernel helper (not in the registry); `DRY_RUN=1` default |
| [`tasks`](../../../packages/mcp/src/tasks/tasks/__init__.py) | generate/read the task-graph demo's SQLite dependency DAG | `seed`, `load`, `frame`, `generate`, `status_of`, `read`/`write`, `SCHEMA` | the data generator for the [task-graph](../task-graph/overview.md) site |
| [`screen`](../../../packages/mcp/src/screen/screen/__init__.py) | native macOS desktop control via CoreGraphics | `capture`, `cursor`, `move`, `click`, `drag`, `write`, `press`, `apps`, `frontmost`, `activate`, `launch`, `terminate` | macOS only; synthetic input needs Accessibility permission |
| [`vmkit`](../../../packages/mcp/src/vmkit/vmkit/__init__.py) | boot and drive a guest VM fully off-screen and screenshot its display | `info`, `install`, `screenshot`, `boot_linux`, `Driver`, `drive`, `run_app`/`run_binary`/`run_oci` | macOS only; needs the `vmkit` binary (`IX_VMKIT_BIN`); a booted `Driver` is a live dashboard resource |
| [`imessage`](../../../packages/mcp/src/imessage/imessage/__init__.py) | read Messages + Contacts into polars and send iMessages | `messages`, `chats`, `contacts`, `send`, `add_contact`/`update_contact`/`delete_contact` | macOS only; reads `~/Library/Messages/chat.db` and the AddressBook DB |

## Bundled from other packages

The kernel namespace also exposes modules the registry lists but `packages/mcp`
does not own; they are baked into the same interpreter from their home packages:
`tui` (PTY driver, `packages/tui-py`), `search` (semantic recall over the fleet
corpus, `packages/search-py`; credential: Mixedbread), and `astlog`/`scipql`/
`flecs_query` (the [code-intel](../../astlog/overview.md) family). The
third-party libraries `numpy`/`polars`/`duckdb`/`httpx`/`matplotlib`/`pypdf`/
`playwright`/`exa_py` (credential: Exa) are import-ready too. These are documented
in their own domains; this page owns only the `packages/mcp/src` providers above.

## Provider groups

- Data and code: `fff`, `view`, `nix` (and bundled `search`, `astlog`,
  `scipql`/`flecs_query`) keep an agent off `ls`/`cat`/`grep`/`rg` and on
  composable polars frames and syntax-highlighted views.
- Distributed: `fleet` is the one cluster surface (Ray for distributed Python,
  Spark Connect for big-data SQL, SSH fan-out, peer live-kernel peek). See the
  fleet deployment note in [common](../overview.md).
- Workflow: `sh`, `worktree`, `mcp_client` run shells, isolate risky work on a
  throwaway branch, and call other MCP servers.
- Accounts (incognito only): `google_auth`, `slack`, `x`.
- macOS native: `screen`, `vmkit`, `imessage` drive the desktop, a guest VM, and
  Messages; `browser`/`x` drive a browser cross-platform.
- Triage: `linear` + its `triage` core + the `nox_autotriage` CI adapter file
  conformance regressions as deduped Linear issues.
