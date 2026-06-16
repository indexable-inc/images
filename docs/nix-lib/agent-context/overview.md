# lib/agent-context: agent context and skills assembly

`lib/agent-context/` assembles the instructions and skills delivered to a coding
agent at session start: the always-on instruction document, the progressive
on-demand skills, the handwritten skills directory, declarative subagents, and
the frontmatter parser they share. `lib/default.nix` imports it as
`ix.agentContext` (`lib/default.nix:120`), `ix.skills`
(`lib/default.nix:121`), and `ix.agents` (`lib/default.nix:122`). It pairs with
the [agents-md](../../agents/agents-md/overview.md) Rust CLI, which writes and
diffs the generated instruction files on disk.

## default.nix: the always/progressive split

`ix.agentContext` (`lib/agent-context/default.nix:1`) parses every file under
`agent-context/sections/`. Each section carries YAML frontmatter naming it and
declaring a disclosure tier (`lib/agent-context/default.nix:38-56`):

- `disclosure: always` sections are concatenated into one small `alwaysDoc` the
  SessionStart hook prints in full (`lib/agent-context/default.nix:76-87`).
- `disclosure: progressive` sections each become a Claude Code skill: only
  `name` + `description` stay always-visible; the body loads on demand.

The always tier size is a build-time invariant: `alwaysDoc` asserts it stays
under `alwaysCharCap` (9000, below Claude's ~10000-char SessionStart limit), so
marking too much `always` fails the build instead of silently truncating
(`lib/agent-context/default.nix:24-28`, `84-87`). Duplicate section names throw
(`lib/agent-context/default.nix:61-71`).

Public surface (`lib/agent-context/default.nix:118-212`): `alwaysCharCap`,
`sections` (keyed by name), `alwaysSections`/`progressiveSections`,
`alwaysDoc`/`alwaysDocLength`, `documents` (the doc rendered per target,
CLAUDE.md and AGENTS.md), `mkProgressiveSkills { pkgs }` (progressive sections ->
skill directories), and `mkApp { pkgs, binary }` (wrap the `agents-md` CLI for
`nix run .#agent-context -- --write`).

## skills.nix: the handwritten skills directory

`ix.skills` (`lib/agent-context/skills.nix:1`) auto-discovers skill directories
under `paths.skills` (each a directory with a `SKILL.md`, optional
`assets/`/`references/`), so adding a directory there is the only step to publish
a shared skill (`lib/agent-context/skills.nix:4-13`). Surface
(`lib/agent-context/skills.nix:64-107`): `sources` (name -> path), `allSkills`,
`profiles.{antithesis,common}` (partitioned by an `antithesis` name prefix), and
`mkSkillsDir { pkgs, names ? allSkills, extraSkills ? {} }`. `mkSkillsDir` builds
one directory of selected skills (merging `mkProgressiveSkills` output via
`extraSkills`), materialized as real directories of real files with no symlinks,
because Claude Code's `/`-autocomplete drops symlinked entries
(`lib/agent-context/skills.nix:47-62`). Unknown names throw.

## agents.nix: declarative subagents

`ix.agents.mkAgentsDir { pkgs, agents }` (`lib/agent-context/agents.nix:47`,
exported `90`) renders declarative subagents to a `.claude/agents/<name>.md`
directory. An agent is `{ frontmatter; body }`: `frontmatter` is an attrset (its
`mcpServers` value comes straight from `ix.mcp.toAgentMcpServers`, so a
subagent's servers are declared from the same registry the wrappers bake,
`lib/agent-context/agents.nix:1-7`), `body` is the markdown system prompt.
Frontmatter is rendered with a fixed lead-key order (name, description, tools,
model, mcpServers), nested values as inline JSON
(`lib/agent-context/agents.nix:15-34`); a `frontmatter.name` must equal the key.
Like skills, output is real files, no symlinks
(`lib/agent-context/agents.nix:60-72`). Used in `lib/per-system.nix` to ship the
`index-action-runner` subagent with a fresh inline `index` MCP server.

## frontmatter.nix: the parser

`import ./frontmatter.nix { inherit lib }` returns a function `text ->
{ frontmatter; body }` (`lib/agent-context/frontmatter.nix:19`). It parses the
constrained shape this repo controls (a leading `---` fence, single-line
`key: value` pairs preserving colons in the value and stripping surrounding
quotes, a closing `---`, then the body), not arbitrary YAML; multi-line block
scalars are unsupported on purpose (`lib/agent-context/frontmatter.nix:2-17`). A
string with no leading fence yields empty frontmatter and the whole string as
body. Used by `default.nix` to parse section files.
