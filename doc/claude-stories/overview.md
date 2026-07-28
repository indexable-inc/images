# claude-stories

`packages/claude-stories` puts an Instagram-style row of "stories" in the Claude
Code status line: each teammate's avatar (initials in a gradient ring) and what
they are working on right now, served peer-to-peer with no central server. It is
a Tokio/axum CLI with four subcommands (`src/main.rs:33-59`).

```
claude-stories publish [--path DIR]            # write your story to a state file
claude-stories serve [--port 4810] [--bind 0.0.0.0]   # serve /story over HTTP
claude-stories render [--port 4810]            # status-line row of peers' stories
claude-stories show [--path DIR]               # print your story as JSON (debug)
```

`DEFAULT_PORT = 4810` (`src/main.rs:24`). Wiring: `render` is the `statusLine`
command, `publish` is a `SessionStart` hook, and `serve` runs once per host
(README); none of that lives in this crate, it is `~/.claude/settings.json`
config.

## Modules

- `story.rs`: the `Story` record, git derivation, and the shared state file.
- `discovery.rs`: peer discovery (tailnet or explicit list).
- `avatar.rs`: text-art avatar rendering for the status line.

## Story and state (`story.rs`)

A `Story` is `{name, repo, branch, subject, ts, url?}` (`src/story.rs:15-30`).
`derive(path)` opens the git repo with `git2` (vendored libgit2, no system
dependency, `Cargo.toml:15-19`) and fills it from `HEAD`: branch shorthand,
latest commit summary as the "what I'm working on" caption, `user.name` (falling
back to the commit author, then `"anonymous"`), the repo basename from the
`origin` remote, and a GitHub https URL when the remote is GitHub
(`src/story.rs:54-115`). Non-GitHub remotes get no URL rather than a guessed one.

State lives at `$XDG_STATE_HOME/claude-stories/story.json` (or
`~/.local/state/...`); `publish` writes it, `serve` reads it (`state_path`,
`write_state`, `read_state`, `src/story.rs:126-152`). `is_fresh(now)` enforces the
24h visibility window `TTL_SECS = 86400` with a checked subtraction so an
`i64::MIN` peer timestamp cannot overflow, and future-dated stories are rejected
(`src/story.rs:32-43`, test at `:169-177`).

## Discovery (`discovery.rs`)

`Discovery::from_env()` chooses a transport (`src/discovery.rs:19-37`):

- `CLAUDE_STORIES_PEERS=host[:port],...` set -> `Peers` (explicit list, for
  testing or off-tailnet use).
- otherwise -> `Tailnet`.

`Tailnet` runs `tailscale status --json` and takes every online peer's IPv4
(`100.x`) address (`tailnet_endpoints`, `src/discovery.rs:79-106`). The tailnet
is already an authenticated, NAT-traversed directory of online devices, so it
doubles as the peer list: no DHT, no bootstrap nodes, no OAuth. `endpoints(port)`
normalizes each peer into a `/story` URL (`story_url`, `src/discovery.rs:48-62`):
bare host -> `http://host:port/story`, `host:port` -> `http://host:port/story`,
full URL passed through.

## Render and serve (`main.rs`)

- **`serve`** is an axum router with one route `GET /story` returning the state
  file as JSON (404 when nothing published, 500 on read error)
  (`src/main.rs:96-118`). It binds `0.0.0.0` by default and is unauthenticated;
  it serves low-sensitivity data (your latest commit subject) and relies on
  tailnet ACLs, so firewall it or `--bind` your tailnet IP on shared networks
  (README "Known limitations").
- **`render`** builds the endpoint list, fans out concurrent fetches with a
  1500ms-timeout `reqwest` client into a `JoinSet`, keeps only fresh stories, and
  prints the avatar row (`src/main.rs:120-148`). A peer that is offline, slow, or
  storyless is simply absent from the row: that is the expected steady state, not
  an error (`src/main.rs:137-138`).

## Avatars (`avatar.rs`)

Claude Code strips Kitty graphics APC sequences from status-line output
(`anthropics/claude-code#39024`), so real profile photos are impossible there;
each avatar is two half-circle glyphs `◖ ◗` around up-to-two uppercase initials,
colored along an Instagram-gradient (`GRADIENT`, `src/avatar.rs:11-18`;
`initials`, `:59-77`). `row(stories)` sorts newest-first, prefixes a
`STORIES` label (with a leading camera glyph) and a dim `+` "your story" bubble,
and joins the avatars (`src/avatar.rs:110-118`).

Security detail: a story's `url` comes from untrusted peer JSON, so it is wrapped
in an OSC 8 hyperlink only when it is plain http(s) and free of control
characters (`is_safe_url`/`link`, `src/avatar.rs:79-93`). Without that guard a
tailnet host could inject terminal escapes into a victim's status line; the test
at `:138-146` pins this.

## How it is built

`default.nix` selects the `claude-stories` binary via
`ix.cargoUnit.selectBinaryWithTests` (flake output `claude-stories`,
`package.nix`):

```
nix build .#claude-stories
```

Tests live inline in each module (`avatar`, `discovery`, `story`) and cover
initials, gradient endpoints, URL safety, URL normalization, and freshness
bounds.

## Known limits

From the README: no real avatars in the status line (escapes stripped);
off-tailnet needs explicit `CLAUDE_STORIES_PEERS`; `serve` is unauthenticated and
binds all interfaces; your story is the last repo you published from, not a live
view of each session's cwd (re-run `publish`, which the `SessionStart` hook does).
