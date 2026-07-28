"""Type stubs for the native PyO3 module.

Hand-maintained to mirror packages/google/py/src/lib.rs. Keep in sync
when changing the binding. Every method returns a native asyncio
awaitable coroutine produced by pyo3-async-runtimes; awaiting it drives
the underlying tokio future.

Result shapes mirror the upstream Google API JSON exactly; the field
layouts are documented in the google-gmail and google-calendar Rust
crates and reflected in ``gmail --json`` / ``gcal --json`` output.
"""

from __future__ import annotations

from collections.abc import Awaitable
from typing import Any

__version__: str

class GmailClient:
    """Gmail client. Reads `GOOGLE_OAUTH_CLIENT_ID`,
    `GOOGLE_OAUTH_CLIENT_SECRET`, and `~/.config/google/token.json`."""

    def __init__(self) -> None: ...
    def search(
        self,
        query: str,
        label_ids: list[str] | None = ...,
        include_spam_trash: bool = ...,
        max_results: int = ...,
    ) -> Awaitable[list[dict[str, Any]]]: ...
    def list_messages(
        self,
        label_ids: list[str] | None = ...,
        include_spam_trash: bool = ...,
        max_results: int = ...,
    ) -> Awaitable[list[dict[str, Any]]]: ...
    def get_message(
        self,
        message_id: str,
        format: str | None = ...,
    ) -> Awaitable[dict[str, Any]]: ...
    def list_threads(
        self,
        query: str | None = ...,
        label_ids: list[str] | None = ...,
        include_spam_trash: bool = ...,
        max_results: int = ...,
    ) -> Awaitable[list[dict[str, Any]]]: ...
    def get_thread(
        self,
        thread_id: str,
        format: str | None = ...,
    ) -> Awaitable[dict[str, Any]]: ...
    def send(
        self,
        to: list[str],
        subject: str,
        body_text: str | None = ...,
        body_html: str | None = ...,
        cc: list[str] | None = ...,
        bcc: list[str] | None = ...,
        thread_id: str | None = ...,
        attachments: list[tuple[str, str, bytes]] | None = ...,
    ) -> Awaitable[dict[str, Any]]: ...
    def create_draft(
        self,
        to: list[str],
        subject: str,
        body_text: str | None = ...,
        body_html: str | None = ...,
        cc: list[str] | None = ...,
        bcc: list[str] | None = ...,
        thread_id: str | None = ...,
        attachments: list[tuple[str, str, bytes]] | None = ...,
    ) -> Awaitable[dict[str, Any]]: ...
    def send_draft(self, draft_id: str) -> Awaitable[dict[str, Any]]: ...
    def list_drafts(self, max_results: int = ...) -> Awaitable[list[dict[str, Any]]]: ...
    def delete_draft(self, draft_id: str) -> Awaitable[None]: ...
    def modify_labels(
        self,
        message_id: str,
        add: list[str] | None = ...,
        remove: list[str] | None = ...,
    ) -> Awaitable[dict[str, Any]]: ...
    def archive(self, message_id: str) -> Awaitable[dict[str, Any]]: ...
    def trash(self, message_id: str) -> Awaitable[None]: ...
    def untrash(self, message_id: str) -> Awaitable[None]: ...
    def mark_read(self, message_id: str) -> Awaitable[dict[str, Any]]: ...
    def mark_unread(self, message_id: str) -> Awaitable[dict[str, Any]]: ...
    def list_labels(self) -> Awaitable[list[dict[str, Any]]]: ...
    def get_attachment(
        self, message_id: str, attachment_id: str
    ) -> Awaitable[bytes]: ...

class CalendarClient:
    """Google Calendar client. Reads the same env vars and token file
    as :class:`GmailClient`."""

    def __init__(self) -> None: ...
    def events(
        self,
        time_min: str | None = ...,
        time_max: str | None = ...,
        text: str | None = ...,
        max_events: int = ...,
        calendar_id: str | None = ...,
    ) -> Awaitable[list[dict[str, Any]]]: ...
    def event(
        self, event_id: str, calendar_id: str | None = ...
    ) -> Awaitable[dict[str, Any]]: ...
    def create_event(
        self,
        summary: str,
        start: str,
        end: str,
        all_day: bool = ...,
        description: str | None = ...,
        location: str | None = ...,
        attendees: list[str] | None = ...,
        notify: str | None = ...,
        calendar_id: str | None = ...,
    ) -> Awaitable[dict[str, Any]]: ...
    def cancel_event(
        self,
        event_id: str,
        calendar_id: str | None = ...,
        notify: str | None = ...,
    ) -> Awaitable[None]: ...
