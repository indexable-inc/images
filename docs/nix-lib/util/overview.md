# lib/util: utility functions

`lib/util/` is the pure cross-cutting helper layer: error helpers, the sanctioned
deep-merge, attrset/list helpers, value encoders, the network endpoint type, the
MCP registry renderers, the secrets surface, the executable writers, and the
bench/artifacts catalogs. Most are imported into `lib/default.nix` and surfaced
on `ix.<name>` (e.g. `ix.errors`, `ix.deepMerge`, `ix.attrs`, `ix.lists`,
`ix.toml`, `ix.mcp`, `ix.relativePath`, `ix.secrets`, `ix.endpoint`,
`ix.mkBenchSuite`, `ix.artifacts`, and the `write*Application` writers,
`lib/default.nix:174-317`, `388-427`).

## errors.nix

`ix.errors`: validate-then-return helpers that throw a fixable message instead
of a deep-eval crash (`lib/util/errors.nix`). `assertEnum { name, value, valid }`
returns `value` or throws listing every valid value
(`lib/util/errors.nix:24-33`); `requireArg { context, args, name }` returns a
required arg or throws naming the helper (`lib/util/errors.nix:52-61`);
`requireAttr { context, attrset, key }` looks up a key or throws listing the
available keys (`lib/util/errors.nix:72-81`). The
[languages](../languages/overview.md) helpers and many builders route bad args
through these.

## deep-merge.nix

`ix.deepMerge`: the single sanctioned recursive attrset merge; the
`no-recursive-update` lint points here (`lib/util/deep-merge.nix:1-5`). Recurses
only when both sides are non-derivation attrsets; lists and derivations are
leaves; one-sided keys pass through. Three exports
(`lib/util/deep-merge.nix:62-84`): `strict` (throws on a leaf collision, naming
the dotted path), `rhs` (rhs wins at a collision), `strictList` (strict fold over
a list, the N-ary shape `discoverModules`/`packageSet` need).

## attrs.nix and lists.nix

- `ix.attrs.flattenToDotted`: collapse a nested attrset to one keyed by
  `.`-joined paths to each leaf (`{ a.b = 1; } => { "a.b" = 1; }`), recursing
  only into plain attrsets (`lib/util/attrs.nix:34-46`). Compose with
  `mapAttrsToList` for `--config a.b=1` flags or dotted env names.
- `ix.lists`: `findDuplicatesBy keyfn list` (keys mapping >1 element) and
  `findDuplicates` (repeated string elements), both sorted, used by the discovery
  and registry duplicate guards (`lib/util/lists.nix:11-19`).

## endpoint.nix

`ix.endpoint { host, port, scheme ? null, path ? "" }` returns a stringifiable
endpoint: it renders to `host:port` in string context (`__toString`) yet exposes
`.host`/`.port`/`.authority`/`.url` (`lib/util/endpoint.nix:24-47`). `ix.endpointOf
node "name"` resolves a peer's declared `ix.networking.expose.<name>` listener
and pairs its port with the node's east-west hostname, so a consumer never reaches
into a sibling's option tree (`lib/util/endpoint.nix:49-62`). This is the
cross-node wiring primitive `expose` (see [image](../image/overview.md)) feeds.

## toml.nix and mcp.nix

- `ix.toml.scalar value`: encode one Nix scalar as the TOML literal a
  `key = value` line expects (bool bare, string JSON-quoted, int/float as-is);
  throws on lists/attrsets (`lib/util/toml.nix:17-26`). Scalars only; use
  `pkgs.formats.toml` for whole files.
- `ix.mcp`: the single source of truth for the MCP servers the house wrappers
  bake in. Define a server once in a neutral shape
  (`{ transport = "stdio"; command; args?; env? }` or `{ transport = "http";
  url }`) and render it per tool: `toClaudeJson` (Claude Code `mcpServers` JSON),
  `toAgentMcpServers` (subagent frontmatter array), `toCodexEntries` (dotted
  `mcp_servers.*` codex `-c` flags), plus `houseServers { indexCommand }`
  (`lib/util/mcp.nix:76-137`). Consumed by the agent wrappers and
  `lib/agent-context/agents.nix`.

## relative-path.nix

`ix.relativePath`: validate option values later joined under a runtime directory
(`lib/util/relative-path.nix`). `isSafe` accepts ordinary relative paths and
rejects empty/absolute/`.`/`..`/repeated-slash; `isSafeName` requires a single
segment; `shellPath`/`shellParent` return shell snippets joining a root
expression with a validated path; `unsafe`/`unsafeNames` filter a list to the
offenders (`lib/util/relative-path.nix:11-38`).

## secrets.nix

`ix.secrets` (`lib/default.nix:117-119`): normalize a secret spec and render it
to each consumer. `normalize` turns a spec into `{ provider, values, refs }`;
the surface exposes the vaultwarden provider (`rbwCheckCommand`,
`rbwMaterializeCommand`) and per-consumer renderers under `consumers`: `vm.refs`,
`nomad` (`envTemplates`/`runCommand`/`renderJob`), and
`kubernetes.renderExternalSecret` (`lib/util/secrets.nix:314-337`). `mkFleet`
normalizes its `secrets` through this (`lib/image/fleet.nix:47-48`).

## writers.nix

The checked executable writers, all curried `pkgs:`
(`lib/util/writers.nix:266-272`). `writeNushellApplication` is the default for
repo commands (Nu syntax checked at build via `nushell --ide-check`);
`writeBashApplication` is the one sanctioned bash escape hatch (runs under
`set -euo pipefail`, checked with `bash -n` + shellcheck,
`lib/util/writers.nix:132-177`); `writePythonApplication` wraps a Python
entrypoint and runs `ty` over it at build time (`lib/util/writers.nix:3-130`);
`writeProcessComposeApplication` wraps a process-compose spec
(`lib/util/writers.nix:179-`). All default `meta.mainProgram`.

## bench.nix and artifacts.nix

- `ix.mkBenchSuite pkgs { name, indexbench, macros ? [], allocCheck ? null,
  runs ? 10 }` (`lib/util/bench.nix:27-47`) declares a continuous-benchmark suite
  against the `indexbench` CLI, returning `{ app; check? }`: `app` is a `nix
  run`-able perf job (timing/RSS not reproducible, so never a flake check);
  `check` (present only with `allocCheck`) is a hermetic flake check asserting
  reproducible allocation counts stay within budget
  (`lib/util/bench.nix:1-22`).
- `ix.artifacts` (`lib/default.nix:312-317`): pinned artifact catalogs surfaced
  to images and presets by name. `attachArtifactSources` wraps catalog entries in
  `fetchurl` derivations; the `minecraft` sub-attrset exposes mod/plugin catalogs,
  server jars by version, and the loader manifests
  (`lib/util/artifacts.nix:159-189`). Presets must consume entries through this
  set rather than inlining URLs and hashes. Consumed by
  [minecraft](../minecraft/overview.md) loaders.
