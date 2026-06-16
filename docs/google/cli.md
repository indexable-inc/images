# google CLIs

The two shell binaries under [google](overview.md): `gcal`
(`packages/google/calendar/cli`, flake `.#gcal`) and `gmail`
(`packages/google/gmail/cli`, flake `.#gmail`). Both are thin argument-shapers
over their [client crate](clients.md) per RFC 0003: the file parses flags and
renders output, the crate owns the API call, OAuth, and error mapping
(`calendar/cli/src/main.rs:1-6`, `gmail/cli/src/main.rs:1-6`). `--json` on any
command emits the crate's wire types verbatim, the same contract the
[MCP tools](mcp.md) and [Python bindings](python.md) return.

Both require `GOOGLE_OAUTH_CLIENT_ID` and `GOOGLE_OAUTH_CLIENT_SECRET` in the
environment. Run `auth` once per workstation; the grant is shared (see
../common.md).

## gcal (`packages/google/calendar/cli/src/main.rs`)

Subcommands (`Command` enum, `main.rs:28-58`):

| subcommand | what | key flags |
| --- | --- | --- |
| `auth` | consent and store the refresh token | `--paste` (headless), `--json` (NDJSON: `{auth_url}` then `{signed_in,scopes,token_path}`) (`main.rs:68-83`) |
| `logout` | delete this host's grant (idempotent) | `--json` (`main.rs:85-91`, `:336-361`) |
| `print-access-token` | mint and print a current access token | `--json` -> `{access_token, expires_in, scopes}` for the Python helper (`main.rs:60-66`, `:238-254`) |
| `list` | events in a window (default now..+7d) | `--calendar`, `--from`, `--to`, `--max` (20), `--query`, `--json` (`main.rs:101-125`) |
| `show <event-id>` | one event | `--calendar`, `--json` (`main.rs:127-138`) |
| `create` | create an event | `--summary`, `--start`, `--end`, `--all-day`, `--description`, `--location`, `--attendee` (repeatable), `--notify`, `--calendar`, `--json` (`main.rs:140-181`) |
| `cancel <event-id>` | cancel/delete an event | `--notify`, `--calendar`, `--json` (`main.rs:183-198`) |

- **Auth consents to the union.** `auth` calls `begin_consent(.., ALL_KNOWN_SCOPES)`
  (`main.rs:274`) then proves the grant with a 1-event probe read before
  reporting success (`main.rs:307-330`). Reads/writes build an authenticator
  scoped to `[EVENTS_SCOPE]` (`main.rs:256-264`).
- **Time parsing.** `--start`/`--end`/`--from`/`--to` accept RFC 3339 with
  offset, local `YYYY-MM-DD HH:MM`, or a bare date; a wall-clock time made
  ambiguous by DST is rejected, not guessed (`main.rs:513-529`). For
  `--all-day`, `--end` is the inclusive last day and the CLI converts to the
  API's exclusive end (`main.rs:484-511`).
- **`--notify`** maps to `SendUpdates` via a `Notify` value-enum
  (`all`/`external-only`/`none`, default `all` matching the Calendar UI)
  (`main.rs:200-219`).

## gmail (`packages/google/gmail/cli/src/main.rs`)

Subcommands (`Command` enum, `main.rs:26-79`; dispatch `main.rs:318-334`):

| subcommand | what | notes |
| --- | --- | --- |
| `auth` | consent and store the refresh token | `--paste`, `--json` (`main.rs:81-96`) |
| `logout` | delete this host's grant | `--json` (`main.rs:98-104`) |
| `list` | messages, most recent first | `-q/--query`, `--label` (repeatable), `--include-spam-trash`, `--max` (20), `--json` (`main.rs:106-127`) |
| `show <message-id>` | one message (headers + body) | `--metadata` (headers only), `--json` (`main.rs:129-139`) |
| `search <query>` | Gmail query search (alias for `list -q`) | `--threads` (group by thread), `--max`, `--json` (`main.rs:141-154`) |
| `send` | compose and send | shared `ComposeArgs`, `--json` (`main.rs:156-192`) |
| `draft create\|update\|send\|list\|delete\|show` | drafts | `DraftCommand` (`main.rs:194-208`) |
| `thread show <thread-id>` | one thread, messages in order | `--metadata`, `--json` (`main.rs:249-265`) |
| `label list\|apply\|remove` | labels on a message | `LabelCommand` (`main.rs:267-290`) |
| `attach get <message-id> <attachment-id>` | download an attachment | `-o/--output` (default stdout) (`main.rs:292-308`) |
| `archive\|trash\|untrash\|mark-read\|mark-unread <message-id>` | single-message mutations | `SingleIdArgs` (`main.rs:69-78`, `:310-314`) |

- **Bodies come from a file or stdin, never argv.** `ComposeArgs` takes
  `--to`/`--cc`/`--bcc` (repeatable), `--subject`, `--body FILE` (`-` for
  stdin), `--html FILE`, `--attach FILE` (repeatable), `--thread`
  (`main.rs:165-192`). Subjects and addresses are argv; the crate's MIME builder
  rejects header injection in them ([clients](clients.md)).
- **`auth` consents to the union.** Like `gcal`, `gmail auth` requests
  `ALL_KNOWN_SCOPES` so one consent covers both products (`main.rs:355`);
  reads/writes use `[GMAIL_MODIFY, GMAIL_SEND]` (`main.rs:339-343`).
- **No `print-access-token`.** That command exists only on `gcal`
  (`grep` confirms `PrintAccessToken` only in `calendar/cli`); both CLIs share
  one token file, so `gcal print-access-token` serves the gmail grant too.

Both binaries are built by `cargoUnit.selectBinaryWithTests` with a
`packageName` distinct from the binary name so package-keyed checks attach
(`calendar/cli/default.nix:5-8`, `gmail/cli/default.nix:5-8`).
