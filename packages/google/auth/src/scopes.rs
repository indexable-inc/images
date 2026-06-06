//! OAuth scope URLs the repo knows about.
//!
//! Adding a new product means adding its scopes here, so the union for the
//! shared consent flow is one place to look at and one place to extend.

/// Calendar v3 events read/write: enough for list/get/create/cancel, without
/// access to calendar settings or the user's calendar list.
pub const CALENDAR_EVENTS: &str = "https://www.googleapis.com/auth/calendar.events";

/// Gmail read + modify (labels, archive, trash, mark read/unread, drafts).
///
/// Strictly supersedes `gmail.readonly`; the repo does not request the
/// readonly scope, because a half-readonly grant is an awkward state for a
/// caller to be in.
pub const GMAIL_MODIFY: &str = "https://www.googleapis.com/auth/gmail.modify";

/// Gmail send. Separate from `GMAIL_MODIFY` because `modify` does not include
/// the right to send mail.
pub const GMAIL_SEND: &str = "https://www.googleapis.com/auth/gmail.send";

/// Every scope this repo's wrappers know about, joined for the shared
/// consent flow.
///
/// `gmail auth` and `gcal auth` both request this union so one consent
/// grants every Google capability the repo exposes today; adding a new
/// product means adding its scope here.
pub const ALL_KNOWN: &[&str] = &[CALENDAR_EVENTS, GMAIL_MODIFY, GMAIL_SEND];
