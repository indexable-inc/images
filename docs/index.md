# index documentation index

From-source documentation for the `index` repo: a shared, open-source Nix + Rust monorepo of developer tools (semantic code search, a PTY terminal driver, agent loops and an MCP server, ready-to-run OCI images, and reusable NixOS modules). The docs are a per-domain wiki: each domain has a `common.md` orientation page plus one directory per package/component. Start at a domain's `common.md`, then drill into the component pages.

## Conventions

- `docs/index.md` is the only bare markdown file at the docs root; `docs/<domain>/common.md` is the only bare file in a domain dir; every other page lives under `docs/<domain>/<component>/`.
- Per-package: every repo-owned package (and every NixOS module/image) has its own component page.
- `genre`: `living` (current-state reference, kept in sync with source), `recipe` (operational procedure), `historical` (frozen dated note).
- `owns`: the source globs a page documents, so a change to that code maps to the page that must be updated.
- The `docs/demo-*` files are README assets, not documentation pages.

## Code and search

### code-intel

AST and semantic code analysis and rewriting: tree-sitter merge/clone/datalog tools, a SCIP find/replace, the Flecs query parser, and shared parsing/highlighting/edit crates.

`owns: packages/ast-merge/** packages/astlog/** packages/clone-detect/** packages/scipql/** packages/flecs-query/** packages/code-highlight/** packages/code-tokenizer/** packages/file-language/** packages/repo-walker/** packages/edit-applier/** packages/llm-clippy/**`

| page | genre | summary |
| --- | --- | --- |
| [code-intel/common.md](code-intel/common.md) | living | Domain orientation: units, shared tree-sitter foundation, analyze/query/rewrite flow, glossary |
| [code-intel/ast-merge/overview.md](code-intel/ast-merge/overview.md) | living | AST-aware git merge driver: GumTree matching + 3DM merge over tree-sitter, line-based fallback, 6 member crates |
| [code-intel/astlog/overview.md](code-intel/astlog/overview.md) | living | Datalog over tree-sitter ASTs: query/scan/fix CLI, lint gate, suppression, Python bindings |
| [code-intel/clone-detect/overview.md](code-intel/clone-detect/overview.md) | living | Type-1/2/3 + sequence clone detection: hashing, scanner, pragmas, clone.toml, SVG badge |
| [code-intel/scipql/overview.md](code-intel/scipql/overview.md) | living | SCIP index to Souffle facts to datalog query and find/replace/rename, toolchain-wrapped CLI |
| [code-intel/flecs-query/overview.md](code-intel/flecs-query/overview.md) | living | Flecs Query Language parser to typed AST; stdio MCP server; Python bindings |
| [code-intel/code-highlight/overview.md](code-intel/code-highlight/overview.md) | living | tree-sitter ANSI syntax highlighter for files and line-numbered snippets |
| [code-intel/code-tokenizer/overview.md](code-intel/code-tokenizer/overview.md) | living | tantivy identifier tokenizer splitting camel/snake/kebab boundaries |
| [code-intel/file-language/overview.md](code-intel/file-language/overview.md) | living | path/name/extension to Language enum, no parser dependencies |
| [code-intel/repo-walker/overview.md](code-intel/repo-walker/overview.md) | living | gitignore-aware text-file iterator skipping binaries |
| [code-intel/edit-applier/overview.md](code-intel/edit-applier/overview.md) | living | apply sorted non-overlapping byte-range edits, render unified diff |
| [code-intel/llm-clippy/overview.md](code-intel/llm-clippy/overview.md) | living | Nix-only clippy fork with restriction lints for LLM-assisted codebases |

### search

Semantic + full-text code search for index: the content-addressed search core, the read CLIs/bindings, the corpus indexer, source adapters, and the Mixedbread/parquet/Iceberg backends.

`owns: packages/search-core/** packages/search/** packages/search-py/** packages/search-eval/** packages/indexer/** packages/file-search/** packages/fff/** packages/source/** packages/sink/** packages/lake/** packages/mixedbread/** packages/polars-mixedbread/** packages/polars-sftp/**`

| page | genre | summary |
| --- | --- | --- |
| [search/common.md](search/common.md) | living | Domain orientation: the corpus/Document model, source -> sink -> store -> search pipeline, dedup/content-addressing invariants, glossary, components table |
| [search/search-core/overview.md](search/search-core/overview.md) | living | Content-addressed core: content hash, manifest, dedup sync, the Store backend trait, MixedbreadStore/MemoryStore, pipeline, config |
| [search/search-core/internals.md](search/search-core/internals.md) | living | Query/projection layer: semantic/grep/recent/stats/ask, projection rules, CodeScope/RenderMode, conversation context, the metadata filter builder, repo slug |
| [search/search/overview.md](search/search/overview.md) | living | Read-only semantic + regex search CLI over the shared store, subcommands/flags/scope selectors, pipe-in rerank mode |
| [search/search-py/overview.md](search/search-py/overview.md) | living | PyO3 binding (import search): async semantic/grep/recent returning polars frames; Linux wheel + MCP bundling |
| [search/search-eval/overview.md](search/search-eval/overview.md) | recipe | Exa-style retrieval + agentic quality harness grading the real search engine over a fixed corpus |
| [search/indexer/overview.md](search/indexer/overview.md) | living | Sync every source into Mixedbread + S3/R2 parquet + the Iceberg lake; scan/consume modes, scan cursor, GC, the services.indexer module |
| [search/source/overview.md](search/source/overview.md) | living | The shared source-meta envelope, Document/Source/SourceAdapter/Reconciler traits, canonical keys, and the body sanitizer |
| [search/source/adapters.md](search/source/adapters.md) | living | Per-adapter table (grain, external_id, tags) for atuin/claude/codex/debug/git/github/journald/linear/slack plus the source-parquet log reader |
| [search/sink/overview.md](search/sink/overview.md) | living | Mixedbread reconcile/replace/apply/GC sink and the S3/R2 parquet corpus-log sink |
| [search/lake/overview.md](search/lake/overview.md) | living | The Iceberg corpus lake: append+tombstone document log, per-slice version fold, snapshot-cursor reads |
| [search/mixedbread/overview.md](search/mixedbread/overview.md) | living | Minimal async Rust client for the Mixedbread vector store API: endpoints, auth, retry/timeouts, filter DSL |
| [search/file-search/overview.md](search/file-search/overview.md) | living | Local BM25 file indexer/searcher on Tantivy: SearchIndex(Reader)/EphemeralSearch, chunking, schema, CLI |
| [search/fff/overview.md](search/fff/overview.md) | living | Third-party fast file-search toolkit packaged for Nix: fff-mcp CLI/MCP server + fff-c cdylib |
| [search/polars-mixedbread/overview.md](search/polars-mixedbread/overview.md) | living | PyO3 scan_mixedbread polars IO source: predicate pushdown, top_k/min_results retrieval depth, typed metadata columns |
| [search/polars-sftp/overview.md](search/polars-sftp/overview.md) | living | PyO3 scan_sftp polars IO source over SFTP: Rust reader + Python plugin, auth/host-key handling |

## Terminal surfaces

### terminal

PTY-backed terminal control: spawn and drive interactive programs (tui + Node/Python bindings), the libghostty-vt VT engine (vt), a session manager (tap), and terminal utilities (terminal-theme, run, kitty).

`owns: packages/tui/** packages/tui-node/** packages/tui-py/** packages/tap/** packages/vt/** packages/terminal-theme/** packages/run/** packages/kitty/**`

| page | genre | summary |
| --- | --- | --- |
| [terminal/common.md](terminal/common.md) | living | Domain orientation: the PTY-driver model, how tui/tap/vt relate, the bindings story, glossary, components table |
| [terminal/tui/overview.md](terminal/tui/overview.md) | living | tui PTY-driver library: TuiManager/TuiInstance, VT-rendered reads, lifecycle, dashboard/publish feature wiring |
| [terminal/tui/internals.md](terminal/tui/internals.md) | living | tui internals: per-child actor task + dedicated !Send VT engine thread, DECCKM cursor-key rewrite, first-paint wait, scrollback walk |
| [terminal/tui-node/overview.md](terminal/tui-node/overview.md) | living | Node N-API binding over tui: Tui/serve/Dashboard, Key/waitFor JS helpers, npm @indexable/tui packaging |
| [terminal/tui-py/overview.md](terminal/tui-py/overview.md) | living | Python PyO3 binding over tui: _tui extension, high-level async Tui API, Playwright-style agent harness, ix-tui wheel |
| [terminal/tap/overview.md](terminal/tap/overview.md) | living | tap terminal session manager (daemon + clients), tap-pty multiplex engine (vt100 mirror + fan-out + resync), tap-protocol wire types |
| [terminal/vt/overview.md](terminal/vt/overview.md) | living | VT engine: ix-vt safe wrapper, ix-vt-sys raw FFI, libghostty-vt C/Zig build and link wiring |
| [terminal/terminal-theme/overview.md](terminal/terminal-theme/overview.md) | living | Light/dark terminal background detection gated on stdout being a TTY |
| [terminal/run/overview.md](terminal/run/overview.md) | recipe | run a command under a recorded PTY session: output.log, asciinema cast, JSONL events, replay scripts, env knobs |
| [terminal/kitty/overview.md](terminal/kitty/overview.md) | living | kitty terminal graphics protocol encoder: transmit/place, Unicode placeholders, base64 chunking |

### dashboard

A live, replayable web canvas plus native-window viewer that aggregates every ix resource-producer socket into one board, built on the engine-free dashboard-core crate.

`owns: packages/dashboard/** packages/dashboard-core/** packages/ix-windows/**`

| page | genre | summary |
| --- | --- | --- |
| [dashboard/common.md](dashboard/common.md) | living | Domain orientation: units, producer->canvas data flow, invariants, glossary, component links |
| [dashboard/dashboard-core/overview.md](dashboard/dashboard-core/overview.md) | living | Engine-free crate: wire types, discovery paths, publish/subscribe socket transport, serve_hub HTTP/SSE server, embedded page, public surface |
| [dashboard/dashboard-core/internals.md](dashboard/dashboard-core/internals.md) | living | The Hub Loro fold (scopes, projections, body-diff thresholds, timestamps) and the durable RecordingStore |
| [dashboard/dashboard/overview.md](dashboard/dashboard/overview.md) | living | Standalone aggregator binary: CLI flags, serve flow, demo producer, flake output .#dashboard |
| [dashboard/ix-windows/overview.md](dashboard/ix-windows/overview.md) | living | Darwin webview consumer: WindowManager reconcile, threading, macOS borderless/120Hz tuning, flake output .#ix-windows |

### media

Terminal-reel recording, macOS screen capture/streaming and HLS ingest, and audio (noise mixing, TTS) tools, four of five driving ffmpeg as the codec engine.

`owns: packages/reel/** packages/screencast/** packages/screencast-ingest/** packages/mynoise/** packages/elevenlabs-say/**`

| page | genre | summary |
| --- | --- | --- |
| [media/common.md](media/common.md) | living | Media domain orientation: units, capture/encode/serve flow, shared ffmpeg/HLS invariants, glossary, components |
| [media/reel/overview.md](media/reel/overview.md) | living | reel: drive real CLIs through the tui PTY driver, rasterize the styled grid, encode animated AVIF/WebP; generates the README demo |
| [media/screencast/overview.md](media/screencast/overview.md) | living | screencast: macOS H.265 (avfoundation+VideoToolbox) desktop capture client streaming fMP4 HLS to ingest (Darwin-only) |
| [media/screencast-ingest/overview.md](media/screencast-ingest/overview.md) | living | screencast-ingest: axum HTTP server storing H.265 HLS per user/session and serving it back for replay/live/indexing |
| [media/mynoise/overview.md](media/mynoise/overview.md) | living | mynoise: resolve, stream, cache, and locally mix myNoise.net band loops with rodio |
| [media/elevenlabs-say/overview.md](media/elevenlabs-say/overview.md) | living | elevenlabs-say: say-style ElevenLabs TTS CLI with WebSocket streaming input and macOS say-compatible flags |

## Agents and orchestration

### agents

Tooling that wraps coding agents (Claude Code, Codex, Pi): session-start context/skills generation, Claude Code hooks, peer status stories, transcript lesson distillation, skill linting, a parallel task runner, and Pi executor harnesses.

`owns: packages/agents-md/** packages/claude-hooks/** packages/claude-stories/** packages/distiller/** packages/skill-lint/** packages/dag-runner/** packages/pi-harnesses/**`

| page | genre | summary |
| --- | --- | --- |
| [agents/common.md](agents/common.md) | living | Domain orientation: units, session-start context loop, transcript->corpus funnel, invariants, glossary, components |
| [agents/agents-md/overview.md](agents/agents-md/overview.md) | living | agents-md CLI: render/diff/check/write generated AGENTS.md+CLAUDE.md from Nix-assembled fragments (flake output agent-context) |
| [agents/claude-hooks/overview.md](agents/claude-hooks/overview.md) | living | One binary, three fail-open Claude Code hooks: session-digest, worktree-guard, prompt-priors |
| [agents/claude-stories/overview.md](agents/claude-stories/overview.md) | living | Status-line teammate stories (avatars + current work) served peer-to-peer over a Tailscale tailnet |
| [agents/distiller/overview.md](agents/distiller/overview.md) | living | Distill ReasoningBank lessons + per-session outcome verdicts from Claude Code transcripts into corpus parquet slices |
| [agents/skill-lint/overview.md](agents/skill-lint/overview.md) | living | Lint and autofix SKILL.md frontmatter with a real YAML parser; rules, severities, exit codes |
| [agents/dag-runner/overview.md](agents/dag-runner/overview.md) | living | Parallel JSON-DAG command runner: validation, scheduling, timeouts/cancellation, output modes, exit codes |
| [agents/pi-harnesses/overview.md](agents/pi-harnesses/overview.md) | living | Pi harness collection: shared builder, model table, engine (pi-harness), base UX, prosecutor |
| [agents/pi-harnesses/beam.md](agents/pi-harnesses/beam.md) | living | pi-beam: bounded beam search over isolated worktree branches scored on ground truth |

### mcp

ix-mcp (ix_notebook_mcp): a Python execution MCP server whose one tool python_exec runs code on a shared persistent IPython kernel preloaded with the repo's developer primitives.

`owns: packages/mcp/**`

| page | genre | summary |
| --- | --- | --- |
| [mcp/common.md](mcp/common.md) | living | Domain orientation: what ix-mcp is, the one-tool/no-install provider model, components table, invariants, glossary |
| [mcp/server/overview.md](mcp/server/overview.md) | living | The host process: ix-mcp CLI/subcommands, stdio+HTTP transports, kernel manager, the three MCP tools, generated instructions, config, the Nix build/flake output |
| [mcp/runtime/overview.md](mcp/runtime/overview.md) | living | The in-kernel runtime: install(), execution flow, jobs/Job, Result, cells/resources, concurrency+capture, the flusher, api()/introspect, the bundled-module registry |
| [mcp/sessions/overview.md](mcp/sessions/overview.md) | living | The SQLite execution store (schema, single-writer, WAL) and the --session reopenable notebook (dill checkpoint + replay-the-gap restore, per-MCP-session namespaces) |
| [mcp/dashboard/overview.md](mcp/dashboard/overview.md) | living | The read-only /api/* data API, the feed embed contract, and the pane_bridge/produce path that publishes the MCP as a producer into the shared Loro dashboard hub |
| [mcp/tool-providers/overview.md](mcp/tool-providers/overview.md) | living | The bundled-module architecture (registry, two-audience Result, async rules, credentials, incognito gate) and a table of every provider under packages/mcp/src |
| [mcp/task-graph/overview.md](mcp/task-graph/overview.md) | living | The standalone Vite+Svelte demo site and its tasks data generator (one SQLite schema, one generator, two consumers) |

### symphony

A boring DAG runtime for deterministic agent workflows: an Elixir/OTP control plane that lowers .sym workflows to a durable IR run graph and drives Codex/Claude turns over a room-server.

`owns: packages/symphony/**`

| page | genre | summary |
| --- | --- | --- |
| [symphony/common.md](symphony/common.md) | living | Domain orientation: deterministic-workflow model, how DSL/engine/pack relate, invariants, glossary, components |
| [symphony/dsl/overview.md](symphony/dsl/overview.md) | living | The .sym language: lexer, grammar, AST, node types (agent/exec/when/every/map), triggers, fields, interpreter |
| [symphony/engine/overview.md](symphony/engine/overview.md) | living | IR run graph and supervised DAG runtime: scheduling, executors, crash recovery, deadlock guard, placement |
| [symphony/engine/contract.md](symphony/engine/contract.md) | living | Engine wire seam: Envelope plus TurnRequest/EngineEvent/AgentTurnResponse and the shared golden fixtures |
| [symphony/pack/overview.md](symphony/pack/overview.md) | living | Workflow-pack format (workflows/, skills/, repositories.yaml), hot-reload catalogs, prompt rendering, the example pack |
| [symphony/operations/overview.md](symphony/operations/overview.md) | recipe | Running it: nix run .#symphony, env vars, state dirs, placements, dashboard at :4040, triggers, the NixOS module |

## Integrations

### integrations

Third-party service API clients and their MCP/CLI/Python surfaces: the google workspace (Gmail + Calendar over one shared installed-app OAuth grant) and github-avatar (commit author to GitHub avatar PNG).

`owns: packages/google/** packages/github-avatar/**`

| page | genre | summary |
| --- | --- | --- |
| [integrations/common.md](integrations/common.md) | living | Domain orientation: units table, the shared-OAuth-grant model, invariants, glossary, components table |
| [integrations/google/overview.md](integrations/google/overview.md) | living | The google workspace orientation, member crates, Nix/flake wiring, and the shared google-auth OAuth flow (PKCE, token store, refresh+rotation, access-token cache) |
| [integrations/google/clients.md](integrations/google/clients.md) | living | Typed google-calendar and google-gmail crates: client methods, wire models, and the Gmail MIME builder |
| [integrations/google/cli.md](integrations/google/cli.md) | living | The gcal and gmail shell CLIs: subcommands and flags |
| [integrations/google/mcp.md](integrations/google/mcp.md) | living | ix-google-mcp stdio MCP server: transport, auth construction, and the 25 calendar_*/mail_* tools |
| [integrations/google/python.md](integrations/google/python.md) | living | ix_google PyO3 async bindings: GmailClient and CalendarClient over the shared grant |
| [integrations/github-avatar/overview.md](integrations/github-avatar/overview.md) | living | github-avatar library: layered commit-author-to-login resolution, login validation, and PNG avatar fetch/re-encode |

## Build and packaging

### nix-build

Nix build-system integration for the index repo: the Cargo-to-Nix per-unit derivation engine, build monitors, OCI image builder, PR rebuild-impact reporter, package registry, and shared CLI build helpers.

`owns: packages/nix-cargo-unit/** packages/nix-web-monitor/** packages/nix-output-monitor/** packages/oci-image-builder/** packages/blast-radius/** packages/registry.nix packages/snix/** packages/build-version/** packages/config-launch/** packages/progress-style/**`

| page | genre | summary |
| --- | --- | --- |
| [nix-build/common.md](nix-build/common.md) | living | Domain orientation: units, Cargo->Nix unit-graph + monitors + image builder + CLI helpers flow, invariants, glossary, components |
| [nix-build/nix-cargo-unit/overview.md](nix-build/nix-cargo-unit/overview.md) | living | Render a Cargo unit graph into per-unit Nix derivations; CLI (render/merge/scan-panics) and the units.nix surface |
| [nix-build/nix-cargo-unit/internals.md](nix-build/nix-cargo-unit/internals.md) | living | Unit identity hashing, graph merge dedup, per-crate source scoping, and the relocation-based panic-reachability scan |
| [nix-build/nix-web-monitor/overview.md](nix-build/nix-web-monitor/overview.md) | living | Run a Nix build with quiet terminal output and a live web monitor; parser + server crates, CLI, and HTTP routes |
| [nix-build/nix-web-monitor/internals.md](nix-build/nix-web-monitor/internals.md) | living | internal-json state machine, msgpack delta transport, out-of-band dependency DAG, copy-size measurement, and the nix-daemon syscall tracer |
| [nix-build/nix-output-monitor/overview.md](nix-build/nix-output-monitor/overview.md) | living | nix-output-monitor (nom) repackaged with a nix-derivation patch so it parses content-addressed derivations |
| [nix-build/oci-image-builder/overview.md](nix-build/oci-image-builder/overview.md) | living | Turn a streamLayeredImage layer plan into an OCI image: legacy one-shot, content-addressed describe/materialize, per-layer sharding, efficiency policy |
| [nix-build/blast-radius/overview.md](nix-build/blast-radius/overview.md) | living | Report how many .#checks.x86_64-linux derivations a PR rebuilds and the changed-input frontier that caused each rebuild |
| [nix-build/registry/overview.md](nix-build/registry/overview.md) | living | Package registry helper: discover every packages/** package.nix and drive the flake/overlay/packageSet/test lists |
| [nix-build/snix/overview.md](nix-build/snix/overview.md) | living | snix default CLI (Rust reimplementation of Nix) built through ix.cargoUnit instead of crate2nix |
| [nix-build/build-version/overview.md](nix-build/build-version/overview.md) | living | Format a binary's --version line from Nix-stamped IX_BUILD_REV/IX_BUILD_EPOCH build metadata |
| [nix-build/config-launch/overview.md](nix-build/config-launch/overview.md) | living | Spec-driven exec launcher: set env/PATH and inject static, argv-conditional, and config-file-gated --config flags, then exec preserving argv0 |
| [nix-build/progress-style/overview.md](nix-build/progress-style/overview.md) | living | Shared indicatif progress-bar and spinner styling for ix CLIs |

### packaging

Repo-owned Nix repackages of third-party tools, each rebuilt with baked-in defaults, patches, or version pins for this fleet.

`owns: packages/btop/** packages/claude-code/** packages/codex/** packages/dia/** packages/launchk/** packages/spark-gluten/** packages/spark-hive/** packages/tonbo-artifacts/** packages/tmux/** packages/vineflower/** packages/yc/**`

| page | genre | summary |
| --- | --- | --- |
| [packaging/common.md](packaging/common.md) | living | Packaging domain orientation: wrapper conventions, structure, flake/registry wiring, pinning/update model, glossary |
| [packaging/btop/overview.md](packaging/btop/overview.md) | living | btop rebuilt via overrideAttrs against a repo-fork source pinned by flake input |
| [packaging/claude-code/overview.md](packaging/claude-code/overview.md) | living | Claude Code CLI wrapper: baked flags/env/settings/MCP/system-prompt/hooks via config-launch, signed-manifest version pin |
| [packaging/codex/overview.md](packaging/codex/overview.md) | living | OpenAI Codex CLI wrapper: forced + soft -c config defaults via config-launch, additive flake-only output |
| [packaging/dia/overview.md](packaging/dia/overview.md) | living | Dia browser .dmg repackaged verbatim with manifest pin and latest-pointer updater, aarch64-darwin only |
| [packaging/launchk/overview.md](packaging/launchk/overview.md) | living | launchd-observer Rust TUI built from a pinned flake-input rev with bindgen and git_version fixups, Darwin only |
| [packaging/spark-gluten/overview.md](packaging/spark-gluten/overview.md) | living | Apache Gluten Velox bundle, native libs autopatchelf'd for NixOS and repacked, x86_64-linux only |
| [packaging/spark-hive/overview.md](packaging/spark-hive/overview.md) | living | Apache Spark hadoop3+Hive distribution, launchers wrapped to pin JDK 17 and TZDIR, x86_64-linux only |
| [packaging/tonbo-artifacts/overview.md](packaging/tonbo-artifacts/overview.md) | living | Prebuilt Tonbo Artifacts CLI binary installed by URL rev pin, x86_64-linux only |
| [packaging/tmux/overview.md](packaging/tmux/overview.md) | living | tmux wrapped via symlinkJoin + wrapProgram to bake a truecolor/mouse/vi config layered under the user's |
| [packaging/vineflower/overview.md](packaging/vineflower/overview.md) | living | Vineflower decompiler release jar with a baked java -jar launcher, inline version pin |
| [packaging/yc/overview.md](packaging/yc/overview.md) | living | YC CLI prebuilt per-platform binaries with manifest pin and latest-pointer updater (no provenance check) |

## Nix infrastructure

### nix-lib

The repo's Nix helper/builder/library API under lib/: the cargo-unit Rust builder, per-language toolchains, OCI image and fleet builders, service and dev-fleet helpers, macOS cross toolchain, Minecraft and agent-context helpers, pure utilities, non-Rust build helpers, and the auto-discovery glue that wires it all into the flake.

`owns: lib/**`

| page | genre | summary |
| --- | --- | --- |
| [nix-lib/common.md](nix-lib/common.md) | living | What lib/ is, how it is assembled in lib/default.nix and exposed via the flake (ix/ixSpecialArgs/ixReturn), the auto-discovery model, glossary, and components table |
| [nix-lib/discovery/overview.md](nix-lib/discovery/overview.md) | living | The discovery/registration glue: registry.nix package index, discoverTree/discoverImages/discoverModules, packageSetFor, overlay, per-system outputs |
| [nix-lib/rust/overview.md](nix-lib/rust/overview.md) | living | cargo-unit core Rust builder: resolve, vendor, policy gates, buildWorkspace unit graph, selectors, prebuilt injection, shared workspace, single-derivation buildPackage |
| [nix-lib/languages/overview.md](nix-lib/languages/overview.md) | living | Per-language toolchain/compiler/interpreter selectors, the common errors-validated pattern, and a per-language table |
| [nix-lib/image/overview.md](nix-lib/image/overview.md) | living | mkImage/mkNonNixImage/evalImageConfig OCI builders, the base platform module, mkFleet fleet eval, mkDev, health-checks |
| [nix-lib/services/overview.md](nix-lib/services/overview.md) | living | portable-services (launchd+systemd), systemd-hardening attrset, mutable-json 3-way-merge home module |
| [nix-lib/dev/overview.md](nix-lib/dev/overview.md) | living | ix.dev.* option surface, agent CLI layer, identity binds, and SMB shared-mount builders for dev fleets |
| [nix-lib/darwin/overview.md](nix-lib/darwin/overview.md) | living | Pinned macOS SDK and the zig + SDK cross toolchain for Linux-to-Darwin Rust builds |
| [nix-lib/minecraft/overview.md](nix-lib/minecraft/overview.md) | living | Typed NBT constructors + format generator, loader module factory, sync-managed wrapper, dimension-type snapshots |
| [nix-lib/agent-context/overview.md](nix-lib/agent-context/overview.md) | living | Always-on instruction doc + progressive skills, skills/agents directory assembly, and the frontmatter parser |
| [nix-lib/util/overview.md](nix-lib/util/overview.md) | living | Pure utilities: errors, deep-merge, attrs/lists, endpoint, toml, mcp, relative-path, secrets, writers, bench, artifacts |
| [nix-lib/build-helpers/overview.md](nix-lib/build-helpers/overview.md) | living | Non-Rust build helpers: bun/npm/uv lock vendoring, JS/Svelte sites, npm-vitest, Go units, Gradle fat-jars, Zig, libghostty-vt |

### modules

Auto-discovered NixOS service modules, opt-in runtime profiles, and the raycast home-manager module under modules/, with the discovery + option-namespace + port-claim conventions that tie them together.

`owns: modules/**`

| page | genre | summary |
| --- | --- | --- |
| [modules/common.md](modules/common.md) | living | Domain orientation: module auto-discovery, option-namespace convention, port-claim/health-check/hardening invariants, services/profiles/home tables, glossary |
| [modules/ci-runner/overview.md](modules/ci-runner/overview.md) | living | Self-hosted GitHub Actions runner pool with warm Nix/Cachix cache (services.ci-runner) |
| [modules/git-clone/overview.md](modules/git-clone/overview.md) | living | Idempotent boot-time gix clone of a repo (services.git-clone) |
| [modules/postgresql/overview.md](modules/postgresql/overview.md) | living | PostgreSQL 18 tuned for Zen 5 with hugepages (services.ix-postgresql) |
| [modules/seaweedfs/overview.md](modules/seaweedfs/overview.md) | living | Single-node S3 object store via weed server -s3 (services.ix-seaweedfs) |
| [modules/observability/overview.md](modules/observability/overview.md) | living | OpenTelemetry Collector + ClickHouse + Grafana telemetry stack (services.ix-observability) |
| [modules/resource-monitor/overview.md](modules/resource-monitor/overview.md) | living | stats-writer Rust crate + Svelte UI for VM usage/billing (services.resource-monitor) |
| [modules/remote-desktop/overview.md](modules/remote-desktop/overview.md) | living | Browser desktop over Xpra HTML5 (services.remote-desktop) |
| [modules/symphony/overview.md](modules/symphony/overview.md) | living | Symphony runtime systemd unit + host codex placement (services.symphony) |
| [modules/ray/overview.md](modules/ray/overview.md) | living | Tailnet Ray cluster + ix-mcp engine for the fleet API (services.ix-ray) |
| [modules/spark/overview.md](modules/spark/overview.md) | living | Standalone Spark 3.5 + Gluten/Velox native engine (services.ix-spark) |
| [modules/minecraft/overview.md](modules/minecraft/overview.md) | living | Loader-agnostic Java Minecraft server core module (services.minecraft) |
| [modules/minecraft/loaders-and-mods.md](modules/minecraft/loaders-and-mods.md) | living | Minecraft loader family (mkMinecraftLoader) and mod/plugin submodules |
| [modules/minecraft-bedrock/overview.md](modules/minecraft-bedrock/overview.md) | living | Bedrock Dedicated Server module (services.minecraft-bedrock) |
| [modules/minestom/overview.md](modules/minestom/overview.md) | living | Minestom fat-jar runtime with ZGC (services.minestom) |
| [modules/velocity/overview.md](modules/velocity/overview.md) | living | Velocity Minecraft proxy with managed plugins/config (services.velocity) |
| [modules/geyser/overview.md](modules/geyser/overview.md) | living | Bedrock-to-Java bridge installed as a Velocity plugin (services.geyser) |
| [modules/floodgate/overview.md](modules/floodgate/overview.md) | living | Floodgate Bedrock auth bridge installed as a Velocity plugin (services.floodgate) |
| [modules/profiles/overview.md](modules/profiles/overview.md) | living | base/jvm runtime profiles and extended-attributes (ix.profiles.*, ix.extendedAttributes) |
| [modules/home/overview.md](modules/home/overview.md) | living | raycast Focus session home-manager module, macOS (programs.raycast.focus) |

### images

Runnable NixOS systems packaged as OCI archives under images/, one thin module per image composing modules/ services and built with nix build .#<name>.

`owns: images/**`

| page | genre | summary |
| --- | --- | --- |
| [images/common.md](images/common.md) | living | What images/ is, the OCI-from-NixOS-closure build model, how an image composes modules, discovery/build, the four families, glossary, components |
| [images/remote-desktop/overview.md](images/remote-desktop/overview.md) | living | Xpra HTML5 browser desktop image (icewm/xterm/firefox), services.remote-desktop wiring and unauthenticated-exposure opt-in |
| [images/development-base/overview.md](images/development-base/overview.md) | living | Default agent dev box: wrapped Claude Code + Codex via lib/dev/agents.nix, build toolchain, one-exception-by-name unfree discipline |
| [images/kernel-dev/overview.md](images/kernel-dev/overview.md) | living | Linux kernel build box; C toolchain plus timer-activated services.git-clone of torvalds/linux to /src/linux |
| [images/neovim-ci/overview.md](images/neovim-ci/overview.md) | living | Neovim upstream CI toolchain image (clang-21, lua/luajit, pynvim, language providers, no service) |
| [images/symphony-codex/overview.md](images/symphony-codex/overview.md) | living | Disposable Symphony agent runner: tmpfs /workspace, wrapped claude, mcp/pi-harness, room-server port claims |
| [images/minecraft/overview.md](images/minecraft/overview.md) | living | Java Minecraft server image with versions.nix per-version variants (minecraft_<ver> + default alias), loader/modCatalog model |
| [images/minecraft-bedrock/overview.md](images/minecraft-bedrock/overview.md) | living | Bedrock Dedicated Server image (native Linux), UDP 19132/19133, tag tracks pinned server version |
| [images/minecraft-status/overview.md](images/minecraft-status/overview.md) | living | Minimal Fabric server used as the ix status/lifecycle canary; firewall off, in-guest SLP health probe |
| [images/minestom/overview.md](images/minestom/overview.md) | living | Minestom hello-world fat-jar server image (no loaders/mods/EULA, ZGC flags, optional YourKit) |
| [images/test-cluster-bootstrap/overview.md](images/test-cluster-bootstrap/overview.md) | living | Bare NixOS bootstrap image (name/tag/hostname only) used to materialize missing fleet nodes |

## VMs and games

### vm-fleet

VM lifecycle (vmkit), fleet convergence (ix-fleet), guest disk images, host/guest demo runners, and live process/kernel debugging (drgn).

`owns: packages/vmkit/** packages/ix-fleet/** packages/chrome-vm/** packages/chrome-vm-image/** packages/vz-linux-guest/** packages/drgn/**`

| page | genre | summary |
| --- | --- | --- |
| [vm-fleet/common.md](vm-fleet/common.md) | living | Domain orientation: units, host/guest/VM-backend relationships, invariants, glossary, components table |
| [vm-fleet/vmkit/overview.md](vm-fleet/vmkit/overview.md) | living | vmkit CLI surface, modules, macOS-guest workflow, entitlement self-signing, MCP cross-ref |
| [vm-fleet/vmkit/linux-guests.md](vm-fleet/vmkit/linux-guests.md) | living | vmkit libkrun Linux-guest backend: EFI/OVMF, GPU/Venus, gvproxy/TSI networking, VZ capture limits, linking |
| [vm-fleet/ix-fleet/overview.md](vm-fleet/ix-fleet/overview.md) | living | Declarative fleet-plan CLI: schema, subcommands, ix SDK control-plane calls, dag-runner fan-out, health checks |
| [vm-fleet/chrome-vm/overview.md](vm-fleet/chrome-vm/overview.md) | living | macOS host runner: build chrome-vm-image, boot under vmkit/libkrun, decode console screenshot, open PNG |
| [vm-fleet/chrome-vm-image/overview.md](vm-fleet/chrome-vm-image/overview.md) | living | aarch64 NixOS guest disk (systemd-repart) with boot-time headless-Chromium screenshot oneshot over the console |
| [vm-fleet/vz-linux-guest/overview.md](vm-fleet/vz-linux-guest/overview.md) | living | aarch64 NixOS GUI guest disk (make-disk-image) booting sway + bossbar-overlay on software graphics for vmkit boot-linux-gui |
| [vm-fleet/drgn/overview.md](vm-fleet/drgn/overview.md) | living | Nix repackaging of drgn v0.2.0 (live process/kernel debugger); build inputs, flake output, platform gating, base-profile use |

### games

Minecraft data/server tooling (NBT, sound, managed-file sync, probe, RCON, hot-reload, a Minestom server) plus reproducible Mojang asset extraction and SQLite-driven Minecraft-style desktop overlays.

`owns: packages/minecraft/** packages/minecraft-assets/** packages/minestom/** packages/bossbar-overlay/**`

| page | genre | summary |
| --- | --- | --- |
| [games/common.md](games/common.md) | living | Domain orientation: units, server-tooling vs desktop-overlay flow, invariants, glossary, components. |
| [games/minecraft/overview.md](games/minecraft/overview.md) | living | minecraft-nbt/sound/sync-managed (Rust), mc-probe/minecraft-rcon (Python), hot-reload-agent (Java) tools. |
| [games/minecraft-assets/overview.md](games/minecraft-assets/overview.md) | living | Reproducible extraction of Mojang GUI textures and bitmap font from the pinned client.jar. |
| [games/minestom/overview.md](games/minestom/overview.md) | living | Example Minestom server fat jar (minestom-hello-server-jar) built via buildGradleFatJar. |
| [games/bossbar-overlay/overview.md](games/bossbar-overlay/overview.md) | living | The boss bar/book/orb desktop overlays, their SQLite contracts, themes, and the bossbar CLI. |
| [games/bossbar-overlay/engine.md](games/bossbar-overlay/engine.md) | living | overlay-core: transparent float window + one wgpu textured-quad pipeline with the bitmap font. |

## Developer tools, SDK, and site

### dev-tools

Miscellaneous first-party developer utilities, benchmarking, and worked examples not belonging to a larger domain: a pretty git-log viewer, an HTTPS reachability prober, a continuous-benchmarking framework, and a standalone CUDA-in-Rust example.

`owns: packages/git-log-pretty/** packages/ix-dev-diagnose/** packages/indexbench/** packages/cuda-hello/**`

| page | genre | summary |
| --- | --- | --- |
| [dev-tools/common.md](dev-tools/common.md) | living | dev-tools domain orientation: units table, cross-component build/packaging invariants, glossary, components table |
| [dev-tools/git-log-pretty/overview.md](dev-tools/git-log-pretty/overview.md) | living | pretty git-log viewer: commits-ahead-of-main file-icon trees, diff subcommand, pager, kitty avatar rendering and GitHub login resolution |
| [dev-tools/ix-dev-diagnose/overview.md](dev-tools/ix-dev-diagnose/overview.md) | living | one-shot HTTPS reachability prober for ix.dev: DNS/TCP/TLS/HTTP probes, dual-trust-store recording verifier, JSON report schema and diagnoses |
| [dev-tools/indexbench/overview.md](dev-tools/indexbench/overview.md) | living | continuous-benchmarking framework: Metric/Run schema, micro/macro/custom harnesses, git-branch and local history stores, CLI, and mkBenchSuite Nix wiring |
| [dev-tools/indexbench/gate.md](dev-tools/indexbench/gate.md) | living | indexbench statistical regression gate: baseline selection, the three regimes, effect size, Mann-Whitney U test, the two CI gates, and reporting |
| [dev-tools/cuda-hello/overview.md](dev-tools/cuda-hello/overview.md) | living | minimal pure-Rust CUDA kernel lowered to PTX via cuda-oxide: why standalone, pinned nightly/flake toolchain, kernel+host code, build/run, status |

### sdk

The Rust, Python, and TypeScript client SDKs for the hosted ix microVM service plus the Nix packaging of the precompiled Python bindings.

`owns: sdk/** packages/ix-sdk-python/**`

| page | genre | summary |
| --- | --- | --- |
| [sdk/common.md](sdk/common.md) | living | What the ix SDKs are, the shared wire protocol/endpoint, the three language surfaces, packaging, glossary, components table |
| [sdk/rust/overview.md](sdk/rust/overview.md) | living | ix-sdk + ix-sdk-wire stub; wire types and the prebuilt-rlib R2 injection/link proof |
| [sdk/python/overview.md](sdk/python/overview.md) | living | ix_sdk async client wrapper over the native _ix_sdk PyO3 extension; Client/Branch/Snapshot/VM surface |
| [sdk/typescript/overview.md](sdk/typescript/overview.md) | living | @indexable/sdk thin wrapper dispatching to napi .node or wasm; Sandbox/Repl + Client/Branch |
| [sdk/packaging/overview.md](sdk/packaging/overview.md) | living | packages/ix-sdk-python: fetches the prebuilt R2 wheel and wraps it as pkgs.ix-sdk-python |

### site

The ix.dev marketing/changelog website: a prerendered SvelteKit + Svelte 5 static site (filterable update log, permalinks, RSS) built by ix.buildSvelteSite and deployed to GitHub Pages.

`owns: site/**`

| page | genre | summary |
| --- | --- | --- |
| [site/common.md](site/common.md) | living | Domain orientation: what the site is, units, data/control flow, invariants, glossary, components table |
| [site/routes/overview.md](site/routes/overview.md) | living | SvelteKit route tree (layout, home feed, [id] permalink, feed.xml) plus app shell, prerender, and base path |
| [site/lib/overview.md](site/lib/overview.md) | living | Shared layer: updates.ts content data layer, UpdateEntry/FilterBar components, tag filter, mdsvex pipeline, styling, tests |
| [site/lib/diagrams.md](site/lib/diagrams.md) | living | The @xyflow/svelte diagram subsystem: DiagramFrame, BoxNode, and per-update diagram wrappers |
| [site/build-deploy/overview.md](site/build-deploy/overview.md) | recipe | Building, previewing, testing, and deploying the site: npm scripts, ix.buildSvelteSite, .#site/.#site-dev, vitest checks, GitHub Pages |

