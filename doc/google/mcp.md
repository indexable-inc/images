# google MCP

`packages/google/mcp` (crate `google-mcp`, binary `ix-google-mcp`, flake
`.#ix-google-mcp`) is a stdio MCP server exposing Gmail and Google Calendar to
an MCP client (Claude, Codex) in one process, sharing one
[`google-auth`](overview.md) grant (`Cargo.toml:6`, `src/main.rs:1-9`). Each
tool is a thin shaper over a [client](clients.md) method and returns the crate's
wire JSON; domain logic and OAuth refresh stay in the core crates (RFC 0003,
`src/tools.rs:1-8`).

## Transport, server info, logging

- **stdio.** `main` builds `GoogleMcp::new()` and serves over `stdio()` via
  `rmcp` (`src/main.rs:28-38`); `rmcp 1.7` with `server`/`macros`/`transport-io`
  (`Cargo.toml:22`).
- **Server info.** Reported name `ix-google-mcp`, version from
  `CARGO_PKG_VERSION`, capability `tools`, with instructions telling the client
  to run `gmail auth` (or `gcal auth`) on the host first
  (`tools.rs:488-504`).
- **Logging to stderr only** (stdout is the MCP wire), filter from
  `IX_GOOGLE_MCP_LOG`, default `info` (`main.rs:40-49`).

## Auth and client construction

`build_clients` reads `GOOGLE_OAUTH_CLIENT_ID`/`GOOGLE_OAUTH_CLIENT_SECRET` and
the on-disk token store, then builds two `Authenticator`s over the full scope
set `[CALENDAR_EVENTS, GMAIL_MODIFY, GMAIL_SEND]`, one per client
(`main.rs:64-84`). The clients share the on-disk refresh token but hold their
own expiry-aware access-token caches, so a refresh in one does not block the
other; if Google rotates the refresh token during one client's refresh, the
other's in-flight refresh can lose the race and fail once, then heal on its next
mint by re-reading the rotated token (`main.rs:51-62`). Auth bootstrap is out of
band: this server only refreshes, never runs consent (`main.rs:11-15`).

## Tools (25)

Tool methods live on `GoogleMcp` behind `#[tool_router]` (`tools.rs:51`);
schemas are derived with `schemars`. Calendar tools keep the `calendar_*` prefix
of the Python `FastMCP` they replace; mail tools use `mail_*` and match the
`superhuman-mail` surface 1:1 so swapping it out is one config change (#599,
`tools.rs:4-8`).

### Calendar (4)

| tool | method -> client call | args |
| --- | --- | --- |
| `calendar_events` | `list_events` | `calendar_id?`, `time_min?`, `time_max?`, `text?`, `max_events?` (50) (`tools.rs:61`, `:513-530`) |
| `calendar_event_get` | `get_event` | `event_id`, `calendar_id?` (`tools.rs:85`) |
| `calendar_event_create` | `create_event` | `summary`, `start`, `end`, `all_day?`, `description?`, `location?`, `attendees?`, `notify?`, `calendar_id?` (`tools.rs:109`, `:539-561`) |
| `calendar_event_cancel` | `cancel_event` | `event_id`, `calendar_id?`, `notify?` (`tools.rs:141`) |

### Gmail search/read (5)

`mail_search` (`list_messages` with `q`), `mail_list_messages` (no free-text
`q`), `mail_get_message`, `mail_list_threads`, `mail_get_thread`
(`tools.rs:168-252`). Search args: `query`/`q`, `label_ids?`,
`include_spam_trash?`, `max_results?` (20). `format?` on get is
`full`(default)/`minimal`/`metadata`/`raw`.

### Gmail send/drafts (7)

`mail_send_message`, `mail_draft_create`, `mail_draft_update`, `mail_draft_get`,
`mail_draft_list`, `mail_draft_delete`, `mail_draft_send`
(`tools.rs:263-353`). Compose args (`MailComposeArgs`, `tools.rs:617-640`):
`to`, `cc?`, `bcc?`, `subject`, `body_text?`, `body_html?`, `thread_id?`,
`attachments?` (each `filename`, `content_type`, `content_base64`, decoded by
`build_outgoing`, `tools.rs:772-811`).

### Gmail single-message mutations (5)

`mail_archive`, `mail_trash`, `mail_untrash`, `mail_mark_read`,
`mail_mark_unread`, each taking `message_id` (`tools.rs:360-420`).

### Gmail labels and attachments (4)

`mail_label_list`, `mail_label_apply` (`message_id`, `label_id`),
`mail_label_remove`, `mail_attachment_get` (`message_id`, `attachment_id` ->
`{content_base64, size}`, standard base64) (`tools.rs:426-484`).

## Argument validation

- `notify` and message `format` default only when absent; an unrecognized value
  is `INVALID_PARAMS`, never a silent "email everyone" or "full bodies"
  (`tools.rs:752-770`, tested `:838-850`).
- All-day event `end` is the inclusive last day at the tool and exclusive on the
  wire, converted by `parse_event_end` (`tools.rs:734-750`, tested `:821-830`).
- Compose requires at least one of `body_text`/`body_html`
  (`tools.rs:774-780`).
- Client errors map to `INTERNAL_ERROR` via `into_tool_error`
  (`tools.rs:701-703`); the underlying `google-auth` message (for example a
  revoked grant or missing scope) is preserved in the text.

Built by `cargoUnit.selectBinaryWithTests` (binary `ix-google-mcp`, package
`google-mcp`, `default.nix:3-6`).
