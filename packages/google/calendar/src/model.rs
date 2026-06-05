//! Wire types for the Calendar v3 `events` resource.
//!
//! Field names mirror the upstream camelCase JSON so the same types serve the
//! HTTP client, the `gcal --json` output, and the MCP tool results: the tool
//! surface and the CLI surface cannot drift (RFC 0003). Only the fields the
//! surfaces actually use are modeled; unknown upstream fields are ignored on
//! read and never invented on write.

use chrono::{DateTime, FixedOffset, NaiveDate};
use serde::{Deserialize, Serialize};

/// When an event starts or ends. Mirrors the `start`/`end` halves of the
/// events resource, where exactly one of `date` (all-day) or `dateTime`
/// (timed) is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EventTime {
    /// An all-day boundary: a civil date with no time or zone. As an `end`,
    /// the date is exclusive (the first day after the event).
    AllDay {
        /// The civil date.
        date: NaiveDate,
    },
    /// A timed boundary: an RFC 3339 instant with offset.
    Timed {
        /// The instant.
        #[serde(rename = "dateTime")]
        date_time: DateTime<FixedOffset>,
        /// IANA zone name; Google attaches it to recurring events.
        #[serde(rename = "timeZone", default, skip_serializing_if = "Option::is_none")]
        time_zone: Option<String>,
    },
}

/// One attendee on an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attendee {
    /// The attendee's email address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Display name, when Google knows one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// `needsAction`, `declined`, `tentative`, or `accepted`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_status: Option<String>,
    /// Whether attendance is marked optional.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

/// A person reference (organizer or creator).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    /// Email address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Display name, when Google knows one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// One calendar event, as returned by the API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    /// Opaque event id, the handle for `get`/`cancel`.
    pub id: String,
    /// `confirmed`, `tentative`, or `cancelled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Event title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Free-text body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Free-text location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Start boundary. Absent only on cancelled stubs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<EventTime>,
    /// End boundary. Absent only on cancelled stubs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<EventTime>,
    /// Attendees, possibly empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<Attendee>,
    /// The event's organizer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organizer: Option<Person>,
    /// Link to the event in the Calendar UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html_link: Option<String>,
    /// Google Meet link, when the event has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hangout_link: Option<String>,
}

/// A new event to create.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventDraft {
    /// Event title.
    pub summary: String,
    /// Free-text body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Free-text location.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Start boundary.
    pub start: EventTime,
    /// End boundary (exclusive date for all-day events).
    pub end: EventTime,
    /// Attendees to invite.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<AttendeeDraft>,
}

/// An attendee on a new event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttendeeDraft {
    /// The attendee's email address.
    pub email: String,
}

/// Who Google emails about a create or cancel, the `sendUpdates` query
/// parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendUpdates {
    /// Notify every attendee.
    All,
    /// Notify only attendees outside the organizer's domain.
    ExternalOnly,
    /// Send no notifications.
    None,
}

impl SendUpdates {
    /// The wire value for the `sendUpdates` query parameter.
    #[must_use]
    pub const fn as_param(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::ExternalOnly => "externalOnly",
            Self::None => "none",
        }
    }
}

/// Selection for [`crate::Client::list_events`]. `None` bounds leave that side
/// of the window open.
#[derive(Debug, Clone)]
pub struct EventQuery {
    /// Lower bound (inclusive) on event end time.
    pub time_min: Option<DateTime<FixedOffset>>,
    /// Upper bound (exclusive) on event start time.
    pub time_max: Option<DateTime<FixedOffset>>,
    /// Free-text search, forwarded as `q`.
    pub text: Option<String>,
    /// Upper bound on returned events; pagination follows `nextPageToken`
    /// until it is reached.
    pub max_events: usize,
}

#[cfg(test)]
mod tests {
    use super::{Event, EventTime};

    #[test]
    fn timed_boundary_round_trips() {
        let json = r#"{"dateTime":"2026-06-05T09:30:00-07:00","timeZone":"America/Los_Angeles"}"#;
        let time: EventTime = serde_json::from_str(json).expect("timed boundary parses");
        let EventTime::Timed {
            date_time,
            time_zone,
        } = &time
        else {
            panic!("expected a timed boundary, got {time:?}");
        };
        assert_eq!(date_time.to_rfc3339(), "2026-06-05T09:30:00-07:00");
        assert_eq!(time_zone.as_deref(), Some("America/Los_Angeles"));
    }

    #[test]
    fn all_day_boundary_parses_even_with_a_zone_attached() {
        // Recurring all-day events carry `timeZone` next to `date`; the
        // untagged enum must still pick the all-day variant.
        let json = r#"{"date":"2026-06-05","timeZone":"Europe/Berlin"}"#;
        let time: EventTime = serde_json::from_str(json).expect("all-day boundary parses");
        assert!(matches!(time, EventTime::AllDay { .. }), "got {time:?}");
    }

    #[test]
    fn cancelled_stub_without_times_parses() {
        // `events.get` on a cancelled event returns only id + status; the
        // model must not require start/end.
        let event: Event = serde_json::from_str(r#"{"id":"abc","status":"cancelled"}"#)
            .expect("cancelled stub parses");
        assert_eq!(event.status.as_deref(), Some("cancelled"));
        assert!(event.start.is_none());
    }
}
