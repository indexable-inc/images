# integrations

Third-party service API clients in the repo, plus the MCP and CLI surfaces
built on top of them. Today that is two things: the [google](google/overview.md)
workspace (one installed-app OAuth flow shared by typed Gmail and Calendar
clients, the `gcal`/`gmail` CLIs, the `ix-google-mcp` server, and PyO3 Python
bindings), and [github-avatar](github-avatar/overview.md) (resolve a git commit
author to a GitHub account and fetch their avatar as PNG). The shared shape of
the domain: a small Rust crate owns the HTTP client, wire types, and error
mapping for one external API, and every user-facing surface over it (CLI, MCP
tool, Python class) stays a thin argument-shaper so they cannot drift
([RFC 0003](../../rfcs/0003-mcp-composable-clis.html), cross-referenced not
documented here).

## Units

| unit | kind | role |
| --- | --- | --- |
| `packages/google/auth` | Rust crate (lib `google_auth`) | installed-app OAuth (PKCE) flow, scoped token store, refresh-with-rotation, expiry-aware access-token cache. See [google](google/overview.md). |
| `packages/google/calendar` | Rust crate (lib `google_calendar`) | typed Calendar v3 events client (list/get/create/cancel). See [google clients](google/clients.md). |
| `packages/google/calendar/cli` | Rust crate `google-calendar-cli`, `gcal` binary, flake `.#gcal` | the `gcal` shell CLI. See [google CLIs](google/cli.md). |
| `packages/google/gmail` | Rust crate (lib `google_gmail`) | typed Gmail v1 client (messages/threads/labels/drafts/send/attachments) + MIME builder. See [google clients](google/clients.md). |
| `packages/google/gmail/cli` | Rust crate `google-gmail-cli`, `gmail` binary, flake `.#gmail` | the `gmail` shell CLI. See [google CLIs](google/cli.md). |
| `packages/google/mcp` | Rust crate `google-mcp`, `ix-google-mcp` binary, flake `.#ix-google-mcp` | stdio MCP server, 25 Gmail + Calendar tools. See [google MCP](google/mcp.md). |
| `packages/google/py` | Rust cdylib `ix-google-py` (PyO3) -> Python `ix_google` | async Python bindings for both clients. See [google Python](google/python.md). |
| `packages/github-avatar` | Rust crate (library, no binary, no flake output) | git commit author -> GitHub login -> avatar PNG. See [github-avatar](github-avatar/overview.md). |

The google units are one Cargo/Nix workspace member tree under `packages/google`
(root `Cargo.toml:44-50`), documented as the single
[google](google/overview.md) component dir. `github-avatar` is a standalone
library crate (`Cargo.toml:43`) consumed by `packages/git-log-pretty` (a
different domain).

## The shared-OAuth-grant model (google)

Everything in the google component shares one auth story (RFC 0003): one team
OAuth client, one per-user consent, one token file, one set of scopes.

- **One OAuth client, from the environment.** `ClientSecrets::from_env`
  (`packages/google/auth/src/lib.rs:89`) reads `GOOGLE_OAUTH_CLIENT_ID` and
  `GOOGLE_OAUTH_CLIENT_SECRET` (`lib.rs:48`, `:51`). For an installed app the
  "secret" is not confidential; it stays in the team vault, out of the repo.
- **One consent, PKCE.** `begin_consent` (`lib.rs:653`) binds a loopback
  listener on `127.0.0.1:0`, builds the consent URL with `access_type=offline`,
  `prompt=consent`, and PKCE `S256` (`lib.rs:665-676`), and exchanges the code
  for an offline refresh token. The headless path
  (`PendingConsent::code_from_redirect_url`, `lib.rs:736`) accepts the redirect
  URL pasted back when the browser cannot reach this host.
- **One token file.** `TokenStore::new` writes
  `~/.config/google/token.json` mode 0600 (`lib.rs:147-153`, `:226-257`), with
  the pre-extraction `~/.config/gcal/token.json` adopted forward on first load
  (`lib.rs:196-215`). A `StoredToken` is `{refresh_token, scopes}`
  (`lib.rs:104`).
- **One scope union.** Consent always requests `ALL_KNOWN_SCOPES`
  (`calendar.events`, `gmail.modify`, `gmail.send`;
  `packages/google/auth/src/scopes.rs:27`), so one `gcal auth` or `gmail auth`
  grants every capability the repo exposes. Each binary then asks
  `Authenticator::new` only for the scopes it needs (`lib.rs:482`), and minting
  fails with `ScopeMissing` if the stored grant lacks one (`lib.rs:606-616`).
  Scope check happens at mint time, not construction.
- **Refresh with rotation, cached.** `Authenticator` mints short-lived access
  tokens from the refresh token, persisting a rotated refresh token when Google
  returns one (`lib.rs:590-597`). The access-token cache is expiry-aware:
  refreshed `ACCESS_TOKEN_REFRESH_MARGIN` (1 min, `lib.rs:67`) before expiry,
  falling back to a 1-hour lifetime when the endpoint omits `expires_in`
  (`lib.rs:325`). A CLI mints one token per run; the MCP server and Python
  bindings hold one `Authenticator` per process and refresh transparently.
- **Wire types are the contract.** The crate model types
  (`packages/google/calendar/src/model.rs`,
  `packages/google/gmail/src/model.rs`) mirror upstream camelCase JSON, and
  `--json`, the MCP tool results, and the Python dicts all emit exactly these,
  so the surfaces cannot drift.

Auth bootstrap is out of band for the long-lived surfaces: `gmail auth` (or
`gcal auth`) on the host mints the refresh token; the MCP server
(`packages/google/mcp/src/main.rs:11-15`) and Python bindings
(`packages/google/py/src/lib.rs:9-13`) only refresh from it, never run consent.

## Invariants

- **Header-injection rejected.** The Gmail MIME builder refuses bare control
  characters and CR/LF in any header value
  (`packages/google/gmail/src/mime.rs:297-302`), so a user-supplied subject or
  address cannot smuggle extra headers.
- **`notify` and `format` never guessed.** Across CLI, MCP, and Python an
  unknown `notify` (who Google emails) or message `format` is a hard error, not
  a silent default to "email everyone" or "full bodies"
  (`packages/google/calendar/src/model.rs:183-197`,
  `packages/google/gmail/src/messages.rs:62-76`).
- **Login validated before any URL use.** `github-avatar` validates a GitHub
  login (1-39 chars, ASCII alnum + hyphen, no leading/trailing hyphen) before
  interpolating it into an avatar URL
  (`packages/github-avatar/src/lib.rs:90-98`).

## Glossary

- **installed-app OAuth**: RFC 6749 + RFC 7636 (PKCE) flow for a desktop app,
  consenting as a person through a team-owned client, no third-party broker.
- **loopback redirect**: the consent redirect lands on a local
  `127.0.0.1:<port>` listener; over SSH/VM the URL is pasted back instead
  (`--paste`).
- **refresh token**: the offline, long-lived credential in `token.json`; minted
  once at consent, occasionally rotated.
- **access token**: the short-lived bearer minted from the refresh token per
  Google call, cached per `Authenticator`.
- **scope union**: the full set (`calendar.events`, `gmail.modify`,
  `gmail.send`) granted by one consent; least privilege is enforced per binary
  at mint time.
- **thin surface**: a CLI/MCP/Python layer that only shapes arguments and
  renders output; all domain logic lives in the core crate (RFC 0003).
- **noreply email**: a `<id>+<login>@users.noreply.github.com` commit address
  that `github-avatar` resolves to a login with no network call.

## Components

| component | page | what |
| --- | --- | --- |
| google | [google/overview.md](google/overview.md) | the workspace, the shared `google-auth` flow, build/flake wiring |
| google clients | [google/clients.md](google/clients.md) | typed `google-calendar` and `google-gmail` crates + MIME builder |
| google CLIs | [google/cli.md](google/cli.md) | `gcal` and `gmail` shell commands and flags |
| google MCP | [google/mcp.md](google/mcp.md) | `ix-google-mcp` stdio server, the 25 tools |
| google Python | [google/python.md](google/python.md) | `ix_google` PyO3 async bindings |
| github-avatar | [github-avatar/overview.md](github-avatar/overview.md) | commit author -> GitHub login -> avatar PNG |
