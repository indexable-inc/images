# google-calendar

Google Calendar for agents and shells: one Rust crate owns the
[Calendar v3 events API](https://developers.google.com/workspace/calendar/api/v3/reference/events)
(list, get, create, cancel). OAuth lives in the shared
[`packages/google/auth`](../auth) crate, which the gmail integration also
uses; three thin surfaces expose this client per
[RFC 0003](../../site/src/lib/rfcs/0003-mcp-composable-clis.svx): the `gcal` CLI
in [`cli/`](./cli), the `calendar_*` tools in the `ix-google-mcp` Rust
server in [`packages/google/mcp`](../mcp), and the
`ix_google.calendar.Client` Python class in
[`packages/google/py`](../py). Tracks
[#643](https://github.com/indexable-inc/index/issues/643); the auth
extraction landed alongside gmail
([#644](https://github.com/indexable-inc/index/issues/644)).

## One-time team setup: the OAuth client

The integration authenticates as a person, through a team-owned OAuth client.
Creating that client happens once, in the Google Cloud console:

1. Pick (or create) a GCP project and enable the Google Calendar API
   (APIs & Services → Library).
2. Configure the OAuth consent screen as Internal, so only org accounts can
   grant access.
3. Create an OAuth client ID of type "Desktop app" (APIs & Services →
   Credentials).
4. Store the client id and secret in the team vault (`rbw`/Vaultwarden, the
   shared-key side of the repo's secrets split). For an installed app the
   "secret" is not confidential in the OAuth sense (Google says so in the
   [installed-app docs](https://developers.google.com/identity/protocols/oauth2/native-app)),
   but it stays out of the repo all the same.

## Authorize, per person

```sh
export GOOGLE_OAUTH_CLIENT_ID="$(rbw get <the client-id entry>)"
export GOOGLE_OAUTH_CLIENT_SECRET="$(rbw get <the client-secret entry>)"
nix run .#gcal -- auth
```

`gcal auth` prints a consent URL and waits on a loopback listener; with a
browser on the same machine the redirect lands there and the flow finishes by
itself. Over SSH or inside a VM the browser cannot reach this host's
`127.0.0.1`, so rerun with `gcal auth --paste`: after consent the browser
shows a connection error on `http://127.0.0.1:…`, and `gcal` reads that full
URL from stdin (paste it in and press enter). Both paths use PKCE and a
per-attempt `state`, and end with a verification read against the API.

The offline refresh token lands in `~/.config/google/token.json` (mode
0600), shared with the gmail integration: one consent flow grants the
union of every scope the repo knows about (`calendar.events`,
`gmail.modify`, `gmail.send`), and running `gmail auth` after `gcal auth`
(or vice versa) is unnecessary. A workstation still holding the legacy
`~/.config/gcal/token.json` is adopted forward transparently on first
use. Revoking the grant at
[myaccount.google.com/permissions](https://myaccount.google.com/permissions)
makes the next call fail with "rerun `gmail auth` (or `gcal auth`)".

## Use it

```sh
gcal list                                  # next 7 days on your calendar
gcal list --from 2026-06-08 --to 2026-06-12 --query standup --json
gcal show <event-id>
gcal create --summary "Design review" --start "2026-06-05 09:30" --end "2026-06-05 10:00" \
  --attendee a@example.com --location "Room 2"
gcal create --summary Offsite --all-day --start 2026-06-10 --end 2026-06-12
gcal cancel <event-id>
```

Times are RFC 3339 with offset, host-local `YYYY-MM-DD HH:MM`, or a bare
date. A wall-clock time that a DST transition makes ambiguous is rejected
rather than guessed. For `--all-day`, `--end` is the last day inclusive; the
crate converts to the API's exclusive end date.

`create` and `cancel` email every attendee by default (`--notify all`, what
the Calendar UI does). Pass `--notify none` while experimenting, or
`--notify external-only`. `--json` on any read/write emits the crate's wire
types verbatim; that output is the contract the MCP tools return.

From the ix-google-mcp side the same capability is `calendar_events`,
`calendar_event_get`, `calendar_event_create`, and `calendar_event_cancel`;
the token file and env credentials must exist on the host running the MCP
server. From Python: `await ix_google.calendar.Client().events()`.

## From the ix-mcp kernel

In an ix-mcp session, `import google_auth` exposes this same grant over the
official `googleapiclient`, with self-service sign-in (no host setup file):

```python
import google_auth

await google_auth.login()            # opens your browser to consent, once
google_auth.status()                 # {"signed_in", "email", "scopes"}
google_auth.gmail().users().messages().send(userId="me", body=msg).execute()
google_auth.calendar().events().list(calendarId="primary").execute()
google_auth.logout()                 # forget this machine's grant
```

`login()` runs the same OAuth flow as `gcal auth` under the hood and stores the
same token file, so a CLI sign-in and a kernel sign-in are interchangeable.
Gmail/Calendar are confined to incognito sessions (never a shared room).

## Layout

- [`src/lib.rs`](./src/lib.rs): the `Client` (list/get/create/cancel,
  pagination, Google error envelope mapping). Re-exports the OAuth types
  from [`google-auth`](../auth).
- [`src/model.rs`](./src/model.rs): wire types; `--json` and the MCP tools
  emit exactly these.
- [`cli/`](./cli): the `gcal` binary, argument shaping only.
- [`tests/client.rs`](./tests/client.rs): wire-level tests against a local
  mock (pagination, request bodies, error envelope, auth-error wrapping).
  OAuth-flow tests live in `google-auth`'s `tests/oauth.rs`.

## Known limitations

- No recurring-event authoring, free/busy queries, or calendar management;
  `list` does expand recurring events into instances.
- One grant per Unix user per host (`~/.config/google/token.json`). Two
  people sharing one VM account would share a calendar identity.
- Human output renders times in the offset the API returns (the calendar's
  timezone), which on a UTC host can differ from your wall clock.
- An access token is minted lazily on first use and refreshed under an
  expiry-aware cache, so a long-lived process (the `ix-google-mcp` server)
  pays Google's token endpoint once per hour, not once per call.
