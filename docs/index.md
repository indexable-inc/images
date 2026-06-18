# index documentation index

From-source documentation for the packages in the `index` repo (a shared, open-source Nix + Rust monorepo of developer tools). One directory per package under `packages/`, each with an `overview.md` (plus concern pages where a package is large). Start at a package's `overview.md`.

## Conventions

- `docs/<package>/` mirrors `packages/<package>/`, one directory per package.
- Each package dir has an `overview.md`; larger packages add concern pages.
- Pages cite real `path:line` references into the package source.
- The `docs/demo-*` files are README assets, not documentation pages.

## Packages

| package | summary |
| --- | --- |
| [ast-merge](ast-merge/overview.md) | `packages/ast-merge` is an AST-aware git merge driver: instead of merging text lines it parses base/left/right with tree-sitter, matches nodes across the three revisions with a GumTree-st... |
| [astlog](astlog/overview.md) | `packages/astlog` runs Datalog over tree-sitter syntax trees: a tree-sitter query match becomes a relation (one row per match, one column per `@capture`), Datalog rules join those relatio... |
| [blast-radius](blast-radius/overview.md) | `packages/blast-radius` reports how many `.#checks.x86_64-linux` derivations a PR would rebuild and which changed inputs caused each rebuild. |
| [bossbar-overlay](bossbar-overlay/overview.md) | `packages/bossbar-overlay` is three transparent, always-on-top, click-through desktop overlays drawn in the Minecraft style with `wgpu`: a boss bar HUD (`bossbar`), an open book (`book`),... |
| [btop](btop/overview.md) | `packages/btop` repackages btop, the resource monitor (CPU, memory, disk, network, process TUI), rebuilt from a repo-owned fork instead of the upstream source. |
| [build-version](build-version/overview.md) | `packages/build-version` is a tiny library crate that formats a binary's `--version` line from build metadata a Nix wrapper stamps into the environment, so every ix tool reports its revis... |
| [chrome-vm](chrome-vm/overview.md) | `packages/chrome-vm` runs headless Chromium inside a real Linux VM on a macOS host and gives the screenshot back, in one command. |
| [chrome-vm-image](chrome-vm-image/overview.md) | `packages/chrome-vm-image` is the raw EFI-bootable aarch64 NixOS disk image that the chrome-vm demo boots under vmkit/libkrun. |
| [claude-code](claude-code/overview.md) | `packages/agent/claude-code` repackages Claude Code, Anthropic's agentic coding CLI, as a prebuilt-binary install with a thick layer of baked-in fleet defaults. |
| [claude-hooks](claude-hooks/overview.md) | `packages/claude-hooks` is one compiled binary with three Claude Code hook subcommands, replacing the old hand-rolled `writeShellScript` hooks in `packages/agent/claude-code`. |
| [claude-stories](claude-stories/overview.md) | `packages/claude-stories` puts an Instagram-style row of "stories" in the Claude Code status line: each teammate's avatar (initials in a gradient ring) and what they are working on right... |
| [clone-detect](clone-detect/overview.md) | `packages/clone-detect` finds duplicated code across a tree. |
| [code-highlight](code-highlight/overview.md) | `packages/code-highlight` is a tree-sitter syntax highlighter that renders a source string (or a line range) as ANSI-colored terminal text. |
| [code-tokenizer](code-tokenizer/overview.md) | `packages/code-tokenizer` is a tantivy tokenizer that splits identifiers the way a code reviewer reads them: on `camelCase`, `snake_case`, `kebab-case`, and any non-alphanumeric run. |
| [codex](codex/overview.md) | `packages/agent/codex` repackages the OpenAI Codex CLI (the nixpkgs `codex` package) with baked-in `-c` config defaults. |
| [config-launch](config-launch/overview.md) | `packages/config-launch` is a spec-driven exec launcher: it reads a JSON spec, sets environment variables and `PATH`, injects CLI flags (static, argv-conditional, and config-file-gated `-... |
| [cuda-hello](cuda-hello/overview.md) | `packages/cuda-hello` is a minimal CUDA kernel written in pure, idiomatic Rust and compiled to PTX with cuda-oxide, NVIDIA's experimental Rust-to-CUDA codegen backend. |
| [dag-runner](dag-runner/overview.md) | `packages/dag-runner` is a tiny task runner: it takes a JSON DAG of shell commands, runs each node as soon as its dependencies finish (so graph shape, not fixed "levels", determines paral... |
| [dashboard](dashboard/overview.md) | `packages/dashboard` is the standalone aggregator: one web canvas for every resource producer on the machine. |
| [dashboard-core](dashboard-core/overview.md) | `packages/dashboard-core` is the engine-free crate every dashboard process links. |
| [dia](dia/overview.md) | `packages/dia` packages Dia, The Browser Company's AI browser, by unpacking its signed, notarized macOS `.dmg` verbatim. |
| [distiller](distiller/overview.md) | `packages/distiller` (`ix-distiller`) distills ReasoningBank-style lessons from local Claude Code transcripts into three artifacts: human-readable facts markdown per `(user, project)`, a... |
| [drgn](drgn/overview.md) | `packages/drgn` is the index repo's Nix repackaging of drgn, Meta's programmable debugger for live processes and kernels. |
| [edit-applier](edit-applier/overview.md) | `packages/edit-applier` applies byte-range edits to source files and renders a unified diff. |
| [elevenlabs-say](elevenlabs-say/overview.md) | `packages/elevenlabs-say` is a `say`-style CLI that speaks text with the ElevenLabs text-to-speech API. |
| [fff](fff/overview.md) | `packages/fff` packages fff, a fast file-search toolkit for humans and AI agents, as a repo Nix package. |
| [file-language](file-language/overview.md) | `packages/file-language` maps a file path, name, or extension to the source `Language` it holds, with no grammar, parser, or highlighting dependencies. |
| [file-search](file-search/overview.md) | `packages/file-search` is a BM25 file indexer and searcher built on Tantivy (`Cargo.toml:6`). |
| [flecs-query](flecs-query/overview.md) | `packages/flecs-query` is a pure-Rust parser for the Flecs Query Language, the string format flecs uses for ECS queries (`Position, [in] Velocity, (ChildOf, $parent)`). |
| [git-log-pretty](git-log-pretty/overview.md) | `packages/git-log-pretty` is a pretty `git log` viewer. |
| [github-avatar](github-avatar/overview.md) | `packages/github-avatar` (crate `github-avatar`) resolves a git commit author to a GitHub account and downloads their avatar as PNG bytes (`Cargo.toml:6`, `src/lib.rs:1-12`). |
| [google](google/overview.md) | `packages/google` is one Cargo/Nix workspace bringing Gmail and Google Calendar into the repo as typed Rust clients with three thin surfaces (shell CLIs, an MCP server, Python bindings) o... |
| [indexbench](indexbench/overview.md) | `packages/indexbench` is a metric-centric continuous-benchmarking framework for the index repo. |
| [indexer](indexer/overview.md) | `packages/indexer` syncs every configured corpus source into Mixedbread (the semantic search index) and a durable corpus log (the S3/R2 parquet archive and/or the Iceberg lake). |
| [ix-dev-diagnose](ix-dev-diagnose/overview.md) | `packages/ix-dev-diagnose` probes `https://ix.dev/` (or any HTTPS URL) from the caller's network path and writes a single JSON diagnostic capturing every layer of the request: system DNS... |
| [ix-fleet](ix-fleet/overview.md) | `packages/ix-fleet` renders and executes declarative **fleet plans**: a single JSON document describes a set of remote ix VMs (nodes) and their images, NixOS switch targets, east-west gro... |
| [ix-sdk-python](ix-sdk-python/overview.md) | `packages/ix-sdk-python` is the Nix package that makes the precompiled Python SDK bindings available in-repo as `pkgs.ix-sdk-python` / `nix build .#ix-sdk-python`. |
| [ix-windows](ix-windows/overview.md) | `packages/ix-windows` renders each live MCP resource as its own floating, blurred overlay webview window that auto-sizes to its content. |
| [kitty](kitty/overview.md) | `packages/kitty` is an encoder for the kitty terminal graphics protocol: it turns image bytes into the `APC _G ... |
| [lake](lake/overview.md) | `packages/lake` (member `lake/iceberg`, crate `lake-iceberg`) is the Iceberg corpus lake: the durable, replayable log under the multi-source search corpus (issue #752), succeeding the ful... |
| [launchk](launchk/overview.md) | `packages/launchk` builds launchk, a cursive (Rust TUI) tool for observing launchd agents and daemons, from source. |
| [llm-clippy](llm-clippy/overview.md) | `packages/llm-clippy` builds a fork of `rust-lang/rust-clippy` carrying extra restriction lints tuned for LLM-assisted codebases. |
| [mcp](mcp/overview.md) | `packages/mcp` is `ix-mcp` (the `ix_notebook_mcp` Python package): a Python execution MCP server. |
| [minecraft](minecraft/overview.md) | `packages/minecraft` is a directory of small, single-purpose Minecraft tools in three languages. |
| [minecraft-assets](minecraft-assets/overview.md) | `packages/minecraft-assets` is a Nix-only package (no Rust, no source code of its own) that produces authentic Minecraft GUI textures and the vanilla bitmap font by extracting them straig... |
| [minestom](minestom/overview.md) | `packages/minestom` packages a minimal, from-scratch Minecraft server built on Minestom, the Java server library. |
| [mixedbread](mixedbread/overview.md) | `packages/mixedbread` (crate `mixedbread`) is a minimal async Rust client for the Mixedbread vector store API. |
| [mynoise](mynoise/overview.md) | `packages/mynoise` plays myNoise.net generators from the CLI by streaming and mixing their band loops locally. |
| [nix-cargo-unit](nix-cargo-unit/overview.md) | `packages/nix-cargo-unit` renders a Cargo unit graph into composable Nix derivations: one `stdenv.mkDerivation` per rustc invocation, wired into a graph that mirrors Cargo's own. |
| [nix-output-monitor](nix-output-monitor/overview.md) | `packages/nix-output-monitor` is the upstream `nix-output-monitor` (`nom`), re-packaged with one patch so it parses content-addressed derivations. |
| [nix-web-monitor](nix-web-monitor/overview.md) | `packages/nix-web-monitor` runs a Nix command with quiet terminal output and a live browser monitor: a build tree, log tail, activity DAG, store-optimisation totals, and a `nix-daemon` sy... |
| [oci-image-builder](oci-image-builder/overview.md) | `packages/oci-image-builder` turns a `dockerTools.streamLayeredImage` layer plan into an OCI image. |
| [pi-harnesses](pi-harnesses/overview.md) | `packages/pi-harnesses` is a collection of Pi-based agent harnesses. |
| [polars-mixedbread](polars-mixedbread/overview.md) | `packages/polars-mixedbread` is a Polars IO source backed by Mixedbread store search. |
| [polars-sftp](polars-sftp/overview.md) | `packages/polars-sftp` is a Polars IO source that reads a remote file over SFTP and hands it back as a lazy `LazyFrame`. |
| [progress-style](progress-style/overview.md) | `packages/progress-style` is a small library crate that owns the shared `indicatif` progress-bar and spinner styling for ix command-line tools, so `search`, `dag-runner`, and future comma... |
| [reel](reel/overview.md) | `packages/reel` records a terminal demo reel "as code": it drives a real CLI session through the tui PTY driver, samples the VT-rendered grid of styled cells over time, rasterizes each fr... |
| [registry.nix](registry.nix/overview.md) | `packages/registry.nix` is the package-registry helper: a single Nix file that discovers every unit under `packages/**` by reading its `package.nix` metadata and produces the per-system l... |
| [repo-walker](repo-walker/overview.md) | `packages/repo-walker` walks a directory tree the way a source-code consumer wants: honor `.gitignore` (plus global, exclude, and `.ignore` files), skip hidden entries, skip known binary... |
| [run](run/overview.md) | `packages/run` executes a command under a recorded PTY session and keeps the output: a full log, a replayable cast, and queryable structured events, while keeping printed output small eno... |
| [scipql](scipql/overview.md) | `packages/scipql` runs Souffle Datalog plus find/replace over a SCIP semantic index. |
| [screencast](screencast/overview.md) | `packages/screencast` is the macOS capture client: it streams a Mac's desktop to a screencast-ingest server as hardware-encoded H.265, so a whole team can push screens to one place that s... |
| [screencast-ingest](screencast-ingest/overview.md) | `packages/screencast-ingest` is the server half of the screencast pipeline: an `axum` HTTP server that receives the fragmented-MP4 HLS streams the screencast client `PUT`s, writes them un... |
| [search](search/overview.md) | `packages/search` is the read-only semantic and regex search CLI over the shared corpus store: one Mixedbread store holding code plus agent/shell history across the fleet. |
| [search-core](search-core/overview.md) | `packages/search-core` is the content-addressed semantic code search core: content hashing, the local manifest, dedup-aware sync, the backend `Store` trait, and the query/projection/filte... |
| [search-eval](search-eval/overview.md) | `packages/search-eval` measures how good `search` is, the way the neural-search community (Exa's "open evals") measures retrieval. |
| [search-py](search-py/overview.md) | `packages/search-py` is the PyO3 binding for `search-core`, imported as `search`. |
| [sink](sink/overview.md) | `packages/sink` is the workspace of search-Document sinks: the write half of the corpus, paired with the source adapters. |
| [skill-lint](skill-lint/overview.md) | `packages/skill-lint` lints and autofixes `SKILL.md` files. |
| [snix](snix/overview.md) | `packages/snix` builds the snix `default` CLI (a Rust reimplementation of Nix, TVL depot `git.snix.dev/snix/snix`) through this repo's nix-cargo-unit engine instead of snix's own crate2ni... |
| [source](source/overview.md) | `packages/source` is the workspace of source adapters that turn each data source (a code checkout's neighbors: Slack, Linear, GitHub, git history, Claude/Codex transcripts, shell history,... |
| [spark-gluten](spark-gluten/overview.md) | `packages/spark-gluten` packages the Apache Gluten Velox-backend bundle for Spark 3.5, patched so its native libraries load on NixOS. |
| [spark-hive](spark-hive/overview.md) | `packages/spark-hive` packages Apache Spark 3.5, the official complete (hadoop3 + Hive) binary distribution, self-contained for NixOS and pinned to JDK 17. |
| [symphony](symphony/overview.md) | Symphony is a boring DAG runtime for deterministic agent workflows. |
| [tap](tap/overview.md) | `packages/tap` is a terminal session manager for tiling-WM users: start a command, detach, reattach later from any terminal, and share a session with others, with no in-terminal tiling la... |
| [terminal-theme](terminal-theme/overview.md) | `packages/terminal-theme` owns one decision shared across the repo's terminal tools: is the terminal background light or dark. |
| [tmux](tmux/overview.md) | `packages/tmux` repackages tmux with a modern default config baked in (truecolor, undercurl, mouse, vi copy mode, sane history and escape-time). |
| [tonbo-artifacts](tonbo-artifacts/overview.md) | `packages/tonbo-artifacts` packages the Tonbo Artifacts CLI, a prebuilt binary served from Tonbo's release host. |
| [tui](tui/overview.md) | `packages/tui` is the repo's flagship PTY primitive: spawn and control multiple interactive terminal programs (gdb, vim, a shell, a REPL) from one process as if a human were typing, and r... |
| [tui-node](tui-node/overview.md) | `packages/tui-node` is the Node.js (N-API) binding for tui: spawn and drive PTY-backed programs (vim, a REPL, a shell) from Node with full VT100 emulation, plus the in-process web dashboard. |
| [tui-py](tui-py/overview.md) | `packages/tui-py` is the Python binding for tui: spawn and control PTY-backed programs from Python with full vt100 emulation, scrollback, NumPy cell access, an in-process web dashboard, a... |
| [vineflower](vineflower/overview.md) | `packages/vineflower` packages Vineflower, the actively-maintained fork of Fernflower, the Java decompiler. |
| [vmkit](vmkit/overview.md) | `packages/vmkit` owns a guest VM's lifecycle from Rust, with one hypervisor backend per host and guest OS: macOS guests on Apple's Virtualization.framework (VZ), Linux guests on libkrun. |
| [vt](vt/overview.md) | `packages/vt` is the VT engine: drive a terminal state machine with raw bytes and snapshot its render state. |
| [vz-linux-guest](vz-linux-guest/overview.md) | `packages/vz-linux-guest` is the raw EFI-bootable aarch64 NixOS disk image that vmkit's `boot-linux-gui` / `drive-linux` path boots under Apple's Virtualization.framework (VZ) on Apple Si... |
| [yc](yc/overview.md) | `packages/yc` packages the Y Combinator CLI (`yc`: search Bookface and chat with the YC Agent from the terminal), installing the upstream prebuilt per-platform binaries. |

