# google-gmail

Gmail for agents and shells: one Rust crate owns the
[Gmail v1 API](https://developers.google.com/gmail/api/reference/rest)
(messages, threads, labels, drafts, send, attachments) and the MIME
builder for outgoing mail. Three thin surfaces expose it per
[RFC 0003](../../../rfcs/0003-mcp-composable-clis.html): the `gmail` CLI
in [`cli/`](./cli), the `mail_*` tools in the `ix-google-mcp` Rust server
in [`packages/google/mcp`](../mcp), and the `ix_google.gmail.Client`
Python class in [`packages/google/py`](../py). Tracks
[#599](https://github.com/indexable-inc/index/issues/599) and
[#644](https://github.com/indexable-inc/index/issues/644).

OAuth is shared with the calendar crate through `google-auth`: one
consent flow per workstation grants the union of every scope the repo
knows about, and the stored token lives in
`~/.config/google/token.json` (mode 0600).

## One-time team setup: the OAuth client

Same client as the calendar crate. Skip if you already followed
[`packages/google/calendar/README.md`](../calendar/README.md).

1. Pick (or create) a GCP project and enable the Gmail API
   (APIs & Services → Library). Enable the Google Calendar API in the
   same project too, so one OAuth client covers both products.
2. Configure the OAuth consent screen as Internal, so only org accounts
   can grant access.
3. Create an OAuth client ID of type "Desktop app" (APIs & Services →
   Credentials).
4. Store the client id and secret in the team vault (`rbw`/Vaultwarden,
   the shared-key side of the repo's secrets split). The "secret" is not
   confidential for an installed app, but it stays out of the repo all
   the same.

## Authorize, per person

```sh
export GOOGLE_OAUTH_CLIENT_ID="$(rbw get <the client-id entry>)"
export GOOGLE_OAUTH_CLIENT_SECRET="$(rbw get <the client-secret entry>)"
nix run .#gmail -- auth
```

`gmail auth` prints a consent URL and waits on a loopback listener; with
a browser on the same machine the redirect lands there and the flow
finishes by itself. Over SSH or inside a VM the browser cannot reach
this host's `127.0.0.1`, so rerun with `gmail auth --paste`: after
consent the browser shows a connection error on `http://127.0.0.1:…`,
and `gmail` reads that full URL from stdin. Both paths use PKCE and a
per-attempt `state`.

The offline refresh token lands in `~/.config/google/token.json` (mode
0600). One token covers calendar and gmail: running `gcal auth` after
`gmail auth` (or vice versa) is unnecessary, and rerunning either one
re-grants both scope sets. Revoking the grant at
[myaccount.google.com/permissions](https://myaccount.google.com/permissions)
makes the next call fail with "rerun `gmail auth`".

## Use it

```sh
gmail list --query 'is:unread newer_than:1d'
gmail show <message-id> --json
gmail search 'from:alice subject:"design review"'
gmail send --to a@example.com --subject "Test" --body /tmp/body.txt --attach /tmp/diff.patch
gmail draft create --to a@example.com --subject "Draft" --body -
gmail label apply <message-id> Label_42
gmail archive <message-id>
gmail attach get <message-id> <attachment-id> -o /tmp/out.pdf
```

Bodies come from a file path or `-` (stdin); never from argv. Subjects
and addresses go on argv. The MIME builder refuses bare control
characters in headers so a user-supplied subject cannot smuggle
additional headers.

`--json` on any read/write emits the crate's wire types verbatim; that
output is the contract the MCP tools and the Python binding return.

From the ix-google-mcp side the surface is `mail_search`,
`mail_get_message`, `mail_send_message`, and so on (twenty-one tools
matching the `superhuman-mail` surface 1:1 first per #599); the token
file and env credentials must exist on the host running the MCP server.
From Python: `await ix_google.gmail.Client().search("from:alice")`.

## Layout

- [`src/lib.rs`](./src/lib.rs): the `Client` (HTTP, error envelope
  mapping, base-URL override).
- [`src/model.rs`](./src/model.rs): wire types; `--json`, the MCP tools,
  and the Python binding all emit exactly these.
- [`src/messages.rs`](./src/messages.rs): list/get/modify-labels/trash
  /untrash/archive/read/unread.
- [`src/threads.rs`](./src/threads.rs): list/get for threads.
- [`src/labels.rs`](./src/labels.rs): list/get for labels.
- [`src/drafts.rs`](./src/drafts.rs): drafts CRUD plus `send_draft` and
  `send_message`.
- [`src/mime.rs`](./src/mime.rs): RFC 5322 + MIME builder with
  header-injection rejection.
- [`src/attachments.rs`](./src/attachments.rs): attachment fetch with
  base64url decoding.
- [`cli/`](./cli): the `gmail` binary, argument shaping only.
- [`tests/client.rs`](./tests/client.rs): wire-level tests against a
  local mock (pagination, request bodies, send round-trip, label
  modify, revoked-grant mapping).

## Known limitations

- No Gmail push (`users.watch` + Pub/Sub) yet. The crate exposes
  `historyId` on each message so a later push-driven loop can resume,
  but the subscription endpoint and fleet-side dispatcher are filed as
  a follow-up to #599.
- One grant per Unix user per host. Two people sharing one VM account
  would share a mailbox identity.
- The MIME builder is deliberately simple: text + html + attachments,
  one level of nesting. Inline images (CID-referenced) and signed
  S/MIME are out of scope.
- Send-rate limits and Google's user-visible quota are not modeled; a
  saturating workflow hits the API's 429 directly.
