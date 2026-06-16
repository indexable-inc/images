# google

`packages/google` is one Cargo/Nix workspace bringing Gmail and Google Calendar
into the repo as typed Rust clients with three thin surfaces (shell CLIs, an MCP
server, Python bindings) over one shared OAuth grant (RFC 0003). This page
covers the orientation and the load-bearing shared crate, `google-auth`. The
other members have their own pages:

- [clients](clients.md): the typed `google-calendar` and `google-gmail` crates
  and the Gmail MIME builder.
- [cli](cli.md): the `gcal` and `gmail` shell binaries.
- [mcp](mcp.md): `ix-google-mcp`, the stdio MCP server and its 25 tools.
- [python](python.md): `ix_google`, the PyO3 async bindings.

Read ../common.md first for the shared-OAuth-grant model that
every member assumes.

## Member crates

| crate | path | lib/bin | flake output |
| --- | --- | --- | --- |
| `google-auth` | `packages/google/auth` | lib `google_auth` | none (library) |
| `google-calendar` | `packages/google/calendar` | lib `google_calendar` | none (library) |
| `google-calendar-cli` | `packages/google/calendar/cli` | bin `gcal` | `.#gcal` |
| `google-gmail` | `packages/google/gmail` | lib `google_gmail` | none (library) |
| `google-gmail-cli` | `packages/google/gmail/cli` | bin `gmail` | `.#gmail` |
| `google-mcp` | `packages/google/mcp` | bin `ix-google-mcp` | `.#ix-google-mcp` |
| `ix-google-py` | `packages/google/py` | cdylib -> Python `ix_google` | none (built as a Python extension) |

Dependency direction: `calendar`, `gmail` -> `auth`; `cli`s -> their client;
`mcp` and `py` -> `auth` + both clients (`packages/google/mcp/Cargo.toml:19-21`,
`packages/google/py/Cargo.toml:17-19`).

## How it is built and wired

Each surface is a `cargoUnit.selectBinaryWithTests` package keyed by its binary,
with `flake = true` in `package.nix` surfacing the flake output:

- `.#gcal`: binary `gcal`, package `google-calendar-cli`
  (`packages/google/calendar/cli/default.nix:3-9`,
  `cli/package.nix:2-6`). `nix run .#gcal -- auth`.
- `.#gmail`: binary `gmail`, package `google-gmail-cli`
  (`packages/google/gmail/cli/default.nix:3-9`). `nix run .#gmail -- auth`.
- `.#ix-google-mcp`: binary `ix-google-mcp`, package `google-mcp`
  (`packages/google/mcp/default.nix:3-6`, `mcp/package.nix:2`).

The library crates (`auth`, `calendar`, `gmail`) and `ix-google-py` are
`inRustWorkspace` units without their own flake output
(`packages/google/auth/package.nix`, `.../py/package.nix`); the Python cdylib is
packaged as an extension module (`crate-type = ["cdylib"]`,
`packages/google/py/Cargo.toml:10`). All members run their package-keyed checks
via `passthruTests`.

## google-auth: the shared OAuth flow

`packages/google/auth` (lib `google_auth`) owns installed-app OAuth (RFC 6749 +
RFC 7636 PKCE) for the repo's Google APIs: a team OAuth client from the
environment, a per-person browser consent on a loopback redirect, an offline
refresh token in a user-only file, and short-lived access tokens minted on
demand (`src/lib.rs:1-18`). No `Debug` on `ClientSecrets`, `StoredToken`, or
`AccessToken` keeps the secret and the tokens out of logs
(`lib.rs:72`, `:114-121`, `:344-352`).

### Public surface

- `ClientSecrets { client_id, client_secret }` and `ClientSecrets::from_env`
  (`lib.rs:73-97`) reading `CLIENT_ID_ENV` / `CLIENT_SECRET_ENV`
  (`GOOGLE_OAUTH_CLIENT_ID`, `GOOGLE_OAUTH_CLIENT_SECRET`, `lib.rs:48`, `:51`).
- `TokenStore` (`lib.rs:124`): `new` -> default `~/.config/google/token.json`
  with the legacy `~/.config/gcal/token.json` migration shim
  (`lib.rs:147-153`); `at`/`with_legacy_path` for tests; `load`, `save`,
  `remove`, `path`. `save` writes mode 0600 and tightens an existing looser
  file (`lib.rs:226-257`); `remove` is idempotent and returns the deleted paths
  (`lib.rs:270-287`).
- `StoredToken { refresh_token, scopes }` (`lib.rs:104`), the persisted grant.
- `scopes` module (`src/scopes.rs`): `CALENDAR_EVENTS`
  (`calendar.events`), `GMAIL_MODIFY` (`gmail.modify`), `GMAIL_SEND`
  (`gmail.send`), and `ALL_KNOWN` (the union, `scopes.rs:27`).
- `begin_consent(secrets, scopes) -> PendingConsent` (`lib.rs:653`).
  `PendingConsent` carries the `auth_url`, and offers `wait_loopback`
  (`lib.rs:703`), `code_from_redirect_url` (`lib.rs:736`, the `--paste` path),
  and `exchange(code) -> StoredToken` (`lib.rs:749`). `AuthCode` is a one-shot
  opaque code (`lib.rs:622`).
- `Authenticator::new(secrets, store, required_scopes)` (`lib.rs:482`);
  `access_token()` (cached, `lib.rs:514`) and `mint_access_token() ->
  AccessToken` (always a network refresh, `lib.rs:528`). `AccessToken { token,
  expires_in, scopes }` (`lib.rs:335`).
- `http_client()` (`lib.rs:853`): one reqwest client builder shared with the
  API clients.
- `Error` (`src/error.rs`): one variant per unavailable prerequisite, each
  message naming the operator's next step: `MissingClientId`/`MissingClientSecret`,
  `NoToken` (names the path and `gmail auth`/`gcal auth`), `TokenRevoked`,
  `ScopeMissing { missing }`, `ConsentDenied`, `StateMismatch`,
  `MissingRefreshToken` (`error.rs:26-157`).

### Flow and invariants

- **Consent URL.** `begin_consent` binds `127.0.0.1:0`, derives the
  `redirect_uri` from the bound port, generates a random `state` and a PKCE
  verifier (two v4 UUIDs = 64 hex chars, inside RFC 7636's 43..=128 window), and
  sets `access_type=offline`, `prompt=consent`, `code_challenge_method=S256`
  (`lib.rs:653-686`). PKCE challenge is verified against the RFC 7636 test
  vector (`lib.rs:866-873`).
- **Redirect intake.** `wait_loopback` accepts on the listener, ignores
  non-redirect probes (favicon, etc.) with a 404, and only returns on a request
  carrying `code`/`error` (`lib.rs:703-727`, `:799-828`); `extract_code` checks
  the `state` matches this attempt and maps Google's `error` to `ConsentDenied`
  (`lib.rs:775-796`).
- **Exchange.** `exchange` POSTs `authorization_code` + `code_verifier`, then
  requires a refresh token in the response or fails `MissingRefreshToken`
  (`lib.rs:749-773`).
- **Refresh + rotation.** `refresh_from` POSTs `grant_type=refresh_token`; on a
  rotated refresh token it immediately persists the replacement
  (`lib.rs:575-604`). A token-endpoint `invalid_grant` on refresh maps to
  `TokenRevoked` (consent gone, re-auth needed), distinct from a code-exchange
  failure (`lib.rs:354-390`).
- **Scope enforcement at mint.** `check_scopes` runs on every mint and returns
  `ScopeMissing` for any required scope absent from the stored grant
  (`lib.rs:606-616`), so a binary requesting `gmail.send` against a
  calendar-only consent fails with a clear message rather than a 403.
- **Expiry-aware cache.** `access_token` serves a cached token while its
  `deadline` is in the future, otherwise mints under the write lock with a
  recheck so concurrent callers serialize on one refresh round-trip
  (`lib.rs:534-571`). Deadline = `now + lifetime - ACCESS_TOKEN_REFRESH_MARGIN`.

### How callers build an authenticator

Each surface constructs `Authenticator::new` with only the scopes it needs:

- `gcal` reads use `&[EVENTS_SCOPE]` (`packages/google/calendar/cli/src/main.rs:258-262`);
  `gmail` uses `&[GMAIL_MODIFY, GMAIL_SEND]`
  (`packages/google/gmail/cli/src/main.rs:339-343`).
- `gcal auth` and `gmail auth` both call `begin_consent(.., ALL_KNOWN_SCOPES)`
  so one consent covers both products
  (`calendar/cli/src/main.rs:274`, `gmail/cli/src/main.rs:352-353`).
- `gcal print-access-token` builds an authenticator with no required scopes and
  calls `mint_access_token`, handing the token (and `expires_in`, `scopes`) to a
  downstream caller via `--json` (`calendar/cli/src/main.rs:238-254`); this is
  the only surface that exposes the raw access token, and it lives on the
  calendar CLI, not gmail.
- `ix-google-mcp` and `ix_google` build authenticators with the full
  `[CALENDAR_EVENTS, GMAIL_MODIFY, GMAIL_SEND]` set
  (`packages/google/mcp/src/main.rs:71`, `packages/google/py/src/lib.rs:114`,
  `:525`).

OAuth-flow tests live in `packages/google/auth/tests/oauth.rs`; the token-store
round-trip, 0600 mode, and idempotent `remove` are unit-tested in
`src/lib.rs:859-957`.
