# google Python bindings

`packages/google/py` (crate `ix-google-py`, cdylib, Python package `ix_google`)
is the PyO3 binding layer over [`google-gmail`](clients.md) and
[`google-calendar`](clients.md), sharing the same [`google-auth`](overview.md)
grant as the CLIs and the [MCP server](mcp.md) (`Cargo.toml:6-10`,
`src/lib.rs:1-13`). Two `Client` classes, one per product; every method is an
`await`-able coroutine bridged from Rust to asyncio via `pyo3-async-runtimes`,
returning dicts shaped exactly like the crate wire types
(`src/lib.rs:1-7`).

## Package layout

- Rust cdylib `crate-type = ["cdylib"]` exporting the `_ix_google` module
  (`src/lib.rs:673-679`), registering `GmailClient` (Python name `GmailClient`),
  `CalendarClient`, and `__version__`.
- Python wrapper `python/ix_google/`: `__init__.py` re-exports the `calendar`
  and `gmail` submodules; `gmail.py`/`calendar.py` re-export the cdylib classes
  as `Client` (`python/ix_google/gmail.py:5`,
  `python/ix_google/calendar.py:5`). Typed via `_ix_google.pyi` + `py.typed`.
- Usage: `await ix_google.gmail.Client().search("from:alice")`,
  `await ix_google.calendar.Client().events()` (`__init__.py:17-24`).

## Auth and conversion

`build_authenticator(scopes)` reads the env credentials and the token store and
builds an `Authenticator` (`src/lib.rs:29-33`). `GmailClient()` requests
`[GMAIL_MODIFY, GMAIL_SEND]` (`lib.rs:114`); `CalendarClient()` requests
`[CALENDAR_EVENTS]` (`lib.rs:525`). Bootstrap is out of band: run `gmail auth`
(or `gcal auth`) on the host first; the constructor only reads the env vars and
`~/.config/google/token.json` (`lib.rs:9-13`). Results are converted with
`pythonize_owned` (serde -> JSON -> Python, `lib.rs:43-50`); Rust errors map to
`RuntimeError`, bad inputs to `ValueError` (`lib.rs:35-41`).

## GmailClient methods (`src/lib.rs:102-473`)

`search(query, label_ids?, include_spam_trash=False, max_results=20)`,
`list_messages(...)`, `get_message(message_id, format?)`,
`list_threads(query?, ...)`, `get_thread(thread_id, format?)`,
`send(to, subject, body_text?, body_html?, cc?, bcc?, thread_id?, attachments?)`,
`create_draft(...)`, `send_draft(draft_id)`, `list_drafts(max_results=20)`,
`delete_draft(draft_id)`, `modify_labels(message_id, add?, remove?)`,
`archive`, `trash`, `untrash`, `mark_read`, `mark_unread` (each by
`message_id`), `list_labels()`, and `get_attachment(message_id, attachment_id)`
which returns raw `bytes` (not base64, unlike the MCP tool) via `PyBytes`
(`lib.rs:457-472`). `attachments` is a list of `(filename, content_type,
content_bytes)` tuples (`lib.rs:241-242`, `:475-510`).

## CalendarClient methods (`src/lib.rs:516-671`)

`events(time_min?, time_max?, text?, max_events=50, calendar_id?)`,
`event(event_id, calendar_id?)`,
`create_event(summary, start, end, all_day=False, description?, location?,
attendees?, notify?, calendar_id?)`, `cancel_event(event_id, calendar_id?,
notify?)`. `time_min`/`time_max`/`start`/`end` are parsed as RFC 3339 (or
`YYYY-MM-DD` for all-day), and all-day `end` is the inclusive last day converted
to the API's exclusive end (`lib.rs:72-96`, `:626-627`).

## Validation (mirrors the other surfaces)

`format` and `notify` default only when absent and reject unknown values
(`parse_message_format`, `parse_send_updates`, `lib.rs:52-70`); `send`/
`create_draft` require at least one of `body_text`/`body_html`
(`build_outgoing`, `lib.rs:486-489`). The crate is `inRustWorkspace` but has no
flake output; it is built as a Python extension module
(`packages/google/py/package.nix:1-4`, `Cargo.toml:20`).
