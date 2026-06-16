# github-avatar

`packages/github-avatar` (crate `github-avatar`) resolves a git commit author to
a GitHub account and downloads their avatar as PNG bytes (`Cargo.toml:6`,
`src/lib.rs:1-12`). It is a library only: no binary, no flake output
(`package.nix:1-4`, no `flake = true`), consumed by `packages/git-log-pretty`
(a different domain; `src/avatar.rs:15` there) to render avatars in a kitty
graphics protocol (`f=100`, which needs PNG).

## Resolution is layered (cheapest first)

`src/lib.rs:3-12` documents the order, so the offline path runs before any
network call:

1. `parse_noreply(email) -> Option<User>` (`lib.rs:68-82`): reads the login
   straight out of a `<...>@users.noreply.github.com` commit email, no network.
   Handles both `49699333+octocat@...` (id+login) and the older
   `octocat@...`, lowercases, and returns `None` unless the embedded login is
   valid (see [Key internals](#key-internals)).
2. `Client::resolve_commit(owner, repo, sha) -> Result<Option<User>>`
   (`lib.rs:176-193`): asks `GET /repos/{owner}/{repo}/commits/{sha}` who
   authored the commit, resolving any linked email. A 404 or 422 is "no answer"
   (`Ok(None)`), not an error.

Once a login is known, `Client::avatar_png(login, size_px) -> Result<Vec<u8>>`
(`lib.rs:209-231`) downloads `https://github.com/{login}.png?size=...` and
returns PNG.

## Public surface

- `User { login }` (`lib.rs:27-30`) and `RepoSlug { owner, repo }`
  (`lib.rs:101-107`).
- `parse_noreply(email)` (`lib.rs:68`), `is_valid_login(login)` (`lib.rs:90`),
  `parse_remote(url)` (`lib.rs:114`): parses a GitHub remote (https/http/ssh/scp
  forms, strips `.git`) into a `RepoSlug`, `None` for non-GitHub remotes so a
  caller can skip the lookup entirely off GitHub (`lib.rs:109-127`).
- `Client::new(token: Option<String>)` (`lib.rs:156`): one connection pool plus
  an optional token (e.g. `GITHUB_TOKEN` or `gh auth token`) used only for the
  API lookup (higher rate limits, private repos); the avatar download needs no
  token.
- `Client::resolve_commit` (`lib.rs:176`), `Client::avatar_png` (`lib.rs:209`).
- `Error`/`Result` (`lib.rs:32-57`): `Request { url, source }`,
  `InvalidLogin { login }`, `Decode { login, source }`, `Encode { login,
  source }`.

## Key internals

- **Login validation (`is_valid_login`, `lib.rs:90-98`).** 1 to 39 chars, ASCII
  alphanumerics and hyphens, no leading/trailing hyphen. The noreply local part
  is attacker-controlled, so validating here is what keeps stray characters
  (slash, `?`, `[bot]`) out of the avatar URL; `parse_noreply` only returns a
  `User` for a valid login (`lib.rs:79-81`, tested `:267-291`).
- **PNG re-encoding (`avatar_png`, `lib.rs:225-230`).** GitHub's `.png`
  endpoint sometimes serves the original upload (JPEG, WebP, GIF). The bytes are
  decoded (`image` with the `png`/`jpeg`/`gif`/`webp` input features,
  `Cargo.toml:15`), resized to an exact `size_px` square with a triangle
  filter, and re-encoded as PNG so the caller always gets PNG. Decode/encode
  failures are `Error::Decode`/`Error::Encode`.
- **HTTP details.** A 5s per-request timeout keeps a stalled api.github.com or
  github.com response from hanging the caller, falling back to an untimed client
  if the builder fails (`lib.rs:160-164`). Every request sends a `User-Agent`
  (`git-log-pretty/<version>`, required by GitHub) and the API calls add
  `Accept: application/vnd.github+json` and `X-GitHub-Api-Version: 2022-11-28`,
  plus bearer auth when a token is set (`lib.rs:21-24`, `:234-247`).
- **Async, rustls.** `reqwest` with `json` + `rustls-tls` (`Cargo.toml:16`);
  both `Client` methods are `async`.

## Build wiring

`inRustWorkspace` with `passthruTests`; root workspace member
(`Cargo.toml:43`), published as a path dependency `github-avatar`
(`Cargo.toml:161`). Unit tests cover noreply parsing (including unsafe-login
rejection), login charset edges, and remote URL forms (`src/lib.rs:250-310`).
