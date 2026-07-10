# google clients

The two typed API crates under [google](overview.md). Each owns the HTTP client,
wire types, and error mapping for one product; OAuth is the shared
[`google-auth`](overview.md) `Authenticator`. The wire types are the contract
the [CLIs](cli.md), [MCP tools](mcp.md), and [Python bindings](python.md) all
emit verbatim (RFC 0003), so the surfaces cannot drift.

## google-calendar (`packages/google/calendar`)

Typed Calendar v3 `events` client: list, get, create, cancel
(`src/lib.rs:1-9`). Re-exports the OAuth types from `google_auth` plus
`EVENTS_SCOPE`/`ALL_KNOWN_SCOPES` (`lib.rs:15-18`).

### Public surface

- `Client::new(auth)` and `with_base_url(auth, url)` (tests)
  (`lib.rs:73-96`). `DEFAULT_BASE_URL = https://www.googleapis.com/calendar/v3`
  (`lib.rs:31`); `PRIMARY_CALENDAR = "primary"` (`lib.rs:34`).
- `list_events(calendar_id, &EventQuery) -> Vec<Event>` (`lib.rs:106`):
  always sets `singleEvents=true` and `orderBy=startTime` (so recurring events
  expand and can be ordered), paginates `nextPageToken` until `max_events`,
  caps each page at `MAX_PAGE_SIZE = 250` (`lib.rs:38`, `:111-151`).
- `get_event(calendar_id, event_id)` (`lib.rs:158`).
- `create_event(calendar_id, &EventDraft, SendUpdates) -> Event` (`lib.rs:175`)
  and `cancel_event(calendar_id, event_id, SendUpdates)` (`lib.rs:201`); both
  append the `sendUpdates` query param.
- Model (`src/model.rs`): `Event`, `EventDraft`, `EventTime` (untagged
  `AllDay { date }` vs `Timed { date_time, time_zone }`, `model.rs:15-33`),
  `Attendee`/`AttendeeDraft`, `Person`, `EventQuery`
  (`time_min`/`time_max`/`text`/`max_events`), and `SendUpdates`
  (`All`/`ExternalOnly`/`None` -> `all`/`externalOnly`/`none`,
  `model.rs:145-165`).
- `Error`/`Result` (`src/error.rs`): API errors carry the message from Google's
  `{"error":{"message":..}}` envelope, decoded by `api_message`
  (`lib.rs:238-277`).

### Key behaviour

- **All-day end is exclusive on the wire.** `EventTime::all_day_end_from_inclusive`
  converts a human inclusive last day to Google's exclusive `end.date`
  (`model.rs:35-45`); the day after the last. The CLI/MCP take the inclusive
  form and convert at the boundary.
- **`SendUpdates` parsing is strict.** `FromStr` accepts `all`,
  `external-only`/`externalOnly`, `none` and errors on anything else, because a
  typo decides who Google emails (`model.rs:183-197`).
- **Lenient reads.** A cancelled-event stub with only `id`+`status` parses
  (start/end optional); an all-day boundary parses even with a stray `timeZone`
  attached (`model.rs:244-252`, `:235-242`).

Tests: wire-level pagination, request bodies, and error-envelope mapping in
`packages/google/calendar/tests/client.rs`.

## google-gmail (`packages/google/gmail`)

Typed Gmail v1 client: messages, threads, labels, drafts, send, attachments,
plus the RFC 5322/MIME builder for outgoing mail (`src/lib.rs:1-16`). Uses
`gmail.modify` + `gmail.send`; both must be on the grant or
`Authenticator::access_token` returns `ScopeMissing` (`lib.rs:14-16`).

### Public surface

- `Client::new(auth)`, `with_base_url`, and `as_user(user_id)` for delegated
  mailboxes (`lib.rs:82-114`). `DEFAULT_BASE_URL =
  https://gmail.googleapis.com/gmail/v1` (`lib.rs:47`); `USER_ME = "me"`
  (`lib.rs:50`). Internal `user_url`/`get`/`post`/`put`/`delete` builders attach
  the bearer token and the `users/{user_id}/...` prefix (`lib.rs:116-158`).
- Messages (`src/messages.rs`): `list_messages(&MessageQuery) ->
  Vec<MessageStub>` (paginated, `MAX_PAGE_SIZE = 500`, `lib.rs:54`),
  `get_message(id, MessageFormat)`, `modify_labels(id, add, remove)`,
  `trash_message`, `untrash_message`, and the helpers `archive_message`
  (remove `INBOX`), `mark_message_read`/`mark_message_unread` (toggle `UNREAD`)
  (`messages.rs:109-239`). `MessageFormat` is `Minimal`/`Full`/`Raw`/`Metadata`
  with strict `FromStr` (`messages.rs:17-76`); `LABEL_INBOX`/`LABEL_UNREAD`
  constants (`messages.rs:11-15`).
- Threads (`src/threads.rs`): `list_threads(&MessageQuery)`,
  `get_thread(id, MessageFormat)` (`threads.rs:36-92`).
- Labels (`src/labels.rs`): `list_labels()`, `get_label(id)`
  (`labels.rs:17-37`); add/remove on a message is `modify_labels`.
- Drafts and send (`src/drafts.rs`): `send_message(&OutgoingMessage)`,
  `create_draft`, `update_draft`, `get_draft`, `list_drafts(max)`,
  `delete_draft`, `send_draft(id)` (`drafts.rs:59-194`).
- Attachments (`src/attachments.rs`): `get_attachment(message_id,
  attachment_id) -> Bytes`, base64url-decoded for the caller
  (`attachments.rs:27-43`).
- Model (`src/model.rs`): `Message`, `MessagePart`/`MessagePartBody`, `Header`
  (with case-insensitive `MessagePart::header`, `model.rs:76-85`), `Thread`,
  `Label`, `Draft`, `MessageQuery`
  (`q`/`label_ids`/`include_spam_trash`/`max_results`), `OutgoingMessage`,
  `Attachment`. `internalDate` round-trips through a typed `DateTime<Utc>` via
  custom serde (`model.rs:217-258`).
- `Error` (`src/error.rs`): API-envelope mapping plus `UnsafeHeader { header }`
  and `Base64 { field }` (`error.rs:65-82`).

### The MIME builder (`src/mime.rs`)

`build_raw(&OutgoingMessage)` produces the base64url RFC 5322 bytes Gmail's
`messages.send`/`drafts.create` take (`mime.rs:33-36`). Layout is decided once
(`Layout::pick`, `mime.rs:74-117`):

- text-only or html-only: a single leaf part, no boundary.
- text + html: `multipart/alternative`.
- any body + attachments: `multipart/mixed` wrapping the body (leaf or nested
  `multipart/alternative`) followed by one base64 attachment per leaf (76-char
  lines per RFC 2045, `mime.rs:260-265`).

`check_safe` rejects any header value with a byte `< 0x20` or `0x7f` (bare
CR/LF, NUL, control chars), so a crafted subject or address cannot inject
headers (`mime.rs:297-302`); this returns `Error::UnsafeHeader` naming the
offending header. Body LF endings are normalized to CRLF on the wire
(`mime.rs:152-168`). Header-injection rejection and the layouts are tested in
`mime.rs:304-419`.

### Known gaps (from README)

No Gmail push (`users.watch` + Pub/Sub) yet; `historyId` is exposed for a later
push loop. The MIME builder is text + html + attachments, one nesting level: no
inline CID images or S/MIME. Quota/rate limits are not modeled (a saturating
workflow hits the API's 429). Tests: `packages/google/gmail/tests/client.rs`.
