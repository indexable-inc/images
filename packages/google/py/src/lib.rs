//! Python bindings for `google-gmail` and `google-calendar`.
//!
//! Two thin `Client` classes, one per product, that share the same
//! [`google_auth::Authenticator`] grant. Every method is an `await`-able
//! coroutine: the underlying Rust call runs on the tokio runtime that
//! pyo3-async-runtimes drives, and the result lands in Python as a dict
//! shaped exactly like the crate's wire types.
//!
//! Auth bootstrap is out of band, like everywhere else in the repo: run
//! `gmail auth` (or `gcal auth`) on the host once to mint the refresh
//! token; the constructor reads `GOOGLE_OAUTH_CLIENT_ID` and
//! `GOOGLE_OAUTH_CLIENT_SECRET` from the environment plus
//! `~/.config/google/token.json` from disk.

use std::sync::Arc;

use google_auth::scopes::{CALENDAR_EVENTS, GMAIL_MODIFY, GMAIL_SEND};
use google_auth::{Authenticator, ClientSecrets, TokenStore};
use google_calendar::{AttendeeDraft, EventDraft, EventQuery, EventTime, SendUpdates};
use google_gmail::{Attachment, MessageFormat, MessageQuery, OutgoingMessage};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;

// ---------------------------------------------------------------------
// Shared auth and conversion helpers
// ---------------------------------------------------------------------

fn build_authenticator(scopes: &[&str]) -> PyResult<Authenticator> {
    let secrets = ClientSecrets::from_env().map_err(into_py_runtime_error)?;
    let store = TokenStore::new().map_err(into_py_runtime_error)?;
    Authenticator::new(secrets, store, scopes).map_err(into_py_runtime_error)
}

fn into_py_runtime_error<E: std::fmt::Display>(error: E) -> PyErr {
    PyRuntimeError::new_err(error.to_string())
}

fn into_py_value_error<E: std::fmt::Display>(error: E) -> PyErr {
    PyValueError::new_err(error.to_string())
}

fn pythonize_owned<T: serde::Serialize>(value: &T) -> PyResult<Py<PyAny>> {
    let json = serde_json::to_value(value).map_err(into_py_runtime_error)?;
    Python::attach(|py| {
        pythonize::pythonize(py, &json)
            .map(|bound| bound.unbind())
            .map_err(into_py_runtime_error)
    })
}

/// `format` defaults to `Full` only when absent; an unrecognized value is
/// an error rather than silently fetching full bodies.
fn parse_message_format(name: Option<&str>) -> PyResult<MessageFormat> {
    name.map_or(Ok(MessageFormat::Full), |value| {
        value
            .parse()
            .map_err(|err| PyValueError::new_err(format!("format: {err}")))
    })
}

/// `notify` defaults to `All` only when absent; an unrecognized value is
/// an error, because it decides who Google emails.
fn parse_send_updates(name: Option<&str>) -> PyResult<SendUpdates> {
    name.map_or(Ok(SendUpdates::All), |value| {
        value
            .parse()
            .map_err(|err| PyValueError::new_err(format!("notify: {err}")))
    })
}

fn parse_event_time(input: &str, all_day: bool, field: &'static str) -> PyResult<EventTime> {
    if all_day {
        let date = input
            .parse()
            .map_err(|err| PyValueError::new_err(format!("{field}: not a date: {err}")))?;
        Ok(EventTime::AllDay { date })
    } else {
        let date_time = chrono::DateTime::parse_from_rfc3339(input)
            .map_err(|err| PyValueError::new_err(format!("{field}: not RFC 3339: {err}")))?;
        Ok(EventTime::Timed {
            date_time,
            time_zone: None,
        })
    }
}

/// Parse the `end` of an event. All-day input is the inclusive last day;
/// Google's all-day `end.date` is exclusive, so convert at this boundary.
fn parse_event_end(input: &str, all_day: bool) -> PyResult<EventTime> {
    match parse_event_time(input, all_day, "end")? {
        EventTime::AllDay { date } => EventTime::all_day_end_from_inclusive(date)
            .ok_or_else(|| PyValueError::new_err(format!("end: no day follows {date}"))),
        timed @ EventTime::Timed { .. } => Ok(timed),
    }
}

// ---------------------------------------------------------------------
// Gmail
// ---------------------------------------------------------------------

#[pyclass(module = "ix_google._ix_google", name = "GmailClient")]
struct GmailClient {
    inner: Arc<google_gmail::Client>,
}

#[pymethods]
impl GmailClient {
    /// Build a client that reads the team OAuth client from the
    /// environment and the refresh token from
    /// `~/.config/google/token.json`.
    #[new]
    fn new() -> PyResult<Self> {
        let auth = build_authenticator(&[GMAIL_MODIFY, GMAIL_SEND])?;
        let client = google_gmail::Client::new(auth).map_err(into_py_runtime_error)?;
        Ok(Self {
            inner: Arc::new(client),
        })
    }

    /// Search messages with the Gmail query syntax.
    #[pyo3(signature = (query, label_ids = None, include_spam_trash = false, max_results = 20))]
    fn search<'py>(
        &self,
        py: Python<'py>,
        query: String,
        label_ids: Option<Vec<String>>,
        include_spam_trash: bool,
        max_results: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        let q = MessageQuery {
            q: Some(query),
            label_ids: label_ids.unwrap_or_default(),
            include_spam_trash,
            max_results,
        };
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let stubs = client
                .list_messages(&q)
                .await
                .map_err(into_py_runtime_error)?;
            pythonize_owned(&stubs)
        })
    }

    /// List messages by filter (no free-text query). With no filter,
    /// returns the most recent messages on the mailbox.
    #[pyo3(signature = (label_ids = None, include_spam_trash = false, max_results = 20))]
    fn list_messages<'py>(
        &self,
        py: Python<'py>,
        label_ids: Option<Vec<String>>,
        include_spam_trash: bool,
        max_results: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        let q = MessageQuery {
            q: None,
            label_ids: label_ids.unwrap_or_default(),
            include_spam_trash,
            max_results,
        };
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let stubs = client
                .list_messages(&q)
                .await
                .map_err(into_py_runtime_error)?;
            pythonize_owned(&stubs)
        })
    }

    /// Fetch one message by id. `format` is `full` (default), `minimal`,
    /// `metadata`, or `raw`.
    #[pyo3(signature = (message_id, format = None))]
    fn get_message<'py>(
        &self,
        py: Python<'py>,
        message_id: String,
        format: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        let fmt = parse_message_format(format.as_deref())?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let message = client
                .get_message(&message_id, fmt)
                .await
                .map_err(into_py_runtime_error)?;
            pythonize_owned(&message)
        })
    }

    /// List threads matching `query` (Gmail search syntax).
    #[pyo3(signature = (query = None, label_ids = None, include_spam_trash = false, max_results = 20))]
    fn list_threads<'py>(
        &self,
        py: Python<'py>,
        query: Option<String>,
        label_ids: Option<Vec<String>>,
        include_spam_trash: bool,
        max_results: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        let q = MessageQuery {
            q: query,
            label_ids: label_ids.unwrap_or_default(),
            include_spam_trash,
            max_results,
        };
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let threads = client
                .list_threads(&q)
                .await
                .map_err(into_py_runtime_error)?;
            pythonize_owned(&threads)
        })
    }

    /// Fetch one thread (with its messages) by id.
    #[pyo3(signature = (thread_id, format = None))]
    fn get_thread<'py>(
        &self,
        py: Python<'py>,
        thread_id: String,
        format: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        let fmt = parse_message_format(format.as_deref())?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let thread = client
                .get_thread(&thread_id, fmt)
                .await
                .map_err(into_py_runtime_error)?;
            pythonize_owned(&thread)
        })
    }

    /// Compose and send a message. `to` is required; at least one of
    /// `body_text` and `body_html` must be set.
    ///
    /// `attachments` is a list of `(filename, content_type, content_bytes)`
    /// tuples.
    #[pyo3(signature = (
        to,
        subject,
        body_text = None,
        body_html = None,
        cc = None,
        bcc = None,
        thread_id = None,
        attachments = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn send<'py>(
        &self,
        py: Python<'py>,
        to: Vec<String>,
        subject: String,
        body_text: Option<String>,
        body_html: Option<String>,
        cc: Option<Vec<String>>,
        bcc: Option<Vec<String>>,
        thread_id: Option<String>,
        attachments: Option<Vec<(String, String, Vec<u8>)>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        let message = build_outgoing(
            to,
            cc.unwrap_or_default(),
            bcc.unwrap_or_default(),
            subject,
            body_text,
            body_html,
            thread_id,
            attachments,
        )?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let sent = client
                .send_message(&message)
                .await
                .map_err(into_py_runtime_error)?;
            pythonize_owned(&sent)
        })
    }

    /// Save a draft.
    #[pyo3(signature = (
        to,
        subject,
        body_text = None,
        body_html = None,
        cc = None,
        bcc = None,
        thread_id = None,
        attachments = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn create_draft<'py>(
        &self,
        py: Python<'py>,
        to: Vec<String>,
        subject: String,
        body_text: Option<String>,
        body_html: Option<String>,
        cc: Option<Vec<String>>,
        bcc: Option<Vec<String>>,
        thread_id: Option<String>,
        attachments: Option<Vec<(String, String, Vec<u8>)>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        let message = build_outgoing(
            to,
            cc.unwrap_or_default(),
            bcc.unwrap_or_default(),
            subject,
            body_text,
            body_html,
            thread_id,
            attachments,
        )?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let draft = client
                .create_draft(&message)
                .await
                .map_err(into_py_runtime_error)?;
            pythonize_owned(&draft)
        })
    }

    /// Send a previously saved draft.
    fn send_draft<'py>(&self, py: Python<'py>, draft_id: String) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let sent = client
                .send_draft(&draft_id)
                .await
                .map_err(into_py_runtime_error)?;
            pythonize_owned(&sent)
        })
    }

    /// List drafts.
    #[pyo3(signature = (max_results = 20))]
    fn list_drafts<'py>(&self, py: Python<'py>, max_results: usize) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let drafts = client
                .list_drafts(max_results)
                .await
                .map_err(into_py_runtime_error)?;
            pythonize_owned(&drafts)
        })
    }

    /// Delete a draft.
    fn delete_draft<'py>(&self, py: Python<'py>, draft_id: String) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            client
                .delete_draft(&draft_id)
                .await
                .map_err(into_py_runtime_error)?;
            Python::attach(|py| Ok(py.None()))
        })
    }

    /// Apply (`add`) and remove (`remove`) labels on a message.
    #[pyo3(signature = (message_id, add = None, remove = None))]
    fn modify_labels<'py>(
        &self,
        py: Python<'py>,
        message_id: String,
        add: Option<Vec<String>>,
        remove: Option<Vec<String>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        let add = add.unwrap_or_default();
        let remove = remove.unwrap_or_default();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let message = client
                .modify_labels(&message_id, &add, &remove)
                .await
                .map_err(into_py_runtime_error)?;
            pythonize_owned(&message)
        })
    }

    /// Archive a message (remove the INBOX label).
    fn archive<'py>(&self, py: Python<'py>, message_id: String) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let message = client
                .archive_message(&message_id)
                .await
                .map_err(into_py_runtime_error)?;
            pythonize_owned(&message)
        })
    }

    /// Move a message to Trash.
    fn trash<'py>(&self, py: Python<'py>, message_id: String) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            client
                .trash_message(&message_id)
                .await
                .map_err(into_py_runtime_error)?;
            Python::attach(|py| Ok(py.None()))
        })
    }

    /// Restore a message from Trash.
    fn untrash<'py>(&self, py: Python<'py>, message_id: String) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            client
                .untrash_message(&message_id)
                .await
                .map_err(into_py_runtime_error)?;
            Python::attach(|py| Ok(py.None()))
        })
    }

    /// Mark a message read (remove UNREAD).
    fn mark_read<'py>(&self, py: Python<'py>, message_id: String) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let message = client
                .mark_message_read(&message_id)
                .await
                .map_err(into_py_runtime_error)?;
            pythonize_owned(&message)
        })
    }

    /// Mark a message unread (add UNREAD).
    fn mark_unread<'py>(&self, py: Python<'py>, message_id: String) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let message = client
                .mark_message_unread(&message_id)
                .await
                .map_err(into_py_runtime_error)?;
            pythonize_owned(&message)
        })
    }

    /// List labels (system + user).
    fn list_labels<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let labels = client.list_labels().await.map_err(into_py_runtime_error)?;
            pythonize_owned(&labels)
        })
    }

    /// Fetch an attachment's bytes.
    fn get_attachment<'py>(
        &self,
        py: Python<'py>,
        message_id: String,
        attachment_id: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let bytes = client
                .get_attachment(&message_id, &attachment_id)
                .await
                .map_err(into_py_runtime_error)?;
            Python::attach(|py| Ok(PyBytes::new(py, &bytes).unbind().into_any()))
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn build_outgoing(
    to: Vec<String>,
    cc: Vec<String>,
    bcc: Vec<String>,
    subject: String,
    body_text: Option<String>,
    body_html: Option<String>,
    thread_id: Option<String>,
    attachments: Option<Vec<(String, String, Vec<u8>)>>,
) -> PyResult<OutgoingMessage> {
    if body_text.is_none() && body_html.is_none() {
        return Err(into_py_value_error(
            "at least one of body_text or body_html is required",
        ));
    }
    let attachments = attachments
        .unwrap_or_default()
        .into_iter()
        .map(|(filename, content_type, content)| Attachment {
            filename,
            content_type,
            content,
        })
        .collect();
    Ok(OutgoingMessage {
        to,
        cc,
        bcc,
        subject,
        body_text,
        body_html,
        thread_id,
        attachments,
    })
}

// ---------------------------------------------------------------------
// Calendar
// ---------------------------------------------------------------------

#[pyclass(module = "ix_google._ix_google", name = "CalendarClient")]
struct CalendarClient {
    inner: Arc<google_calendar::Client>,
}

#[pymethods]
impl CalendarClient {
    #[new]
    fn new() -> PyResult<Self> {
        let auth = build_authenticator(&[CALENDAR_EVENTS])?;
        let client = google_calendar::Client::new(auth).map_err(into_py_runtime_error)?;
        Ok(Self {
            inner: Arc::new(client),
        })
    }

    /// List events on `calendar_id` in a window.
    #[pyo3(signature = (
        time_min = None,
        time_max = None,
        text = None,
        max_events = 50,
        calendar_id = None,
    ))]
    fn events<'py>(
        &self,
        py: Python<'py>,
        time_min: Option<String>,
        time_max: Option<String>,
        text: Option<String>,
        max_events: usize,
        calendar_id: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        let calendar = calendar_id.unwrap_or_else(|| google_calendar::PRIMARY_CALENDAR.to_owned());
        let time_min = match time_min {
            Some(input) => {
                Some(chrono::DateTime::parse_from_rfc3339(&input).map_err(into_py_value_error)?)
            }
            None => None,
        };
        let time_max = match time_max {
            Some(input) => {
                Some(chrono::DateTime::parse_from_rfc3339(&input).map_err(into_py_value_error)?)
            }
            None => None,
        };
        let query = EventQuery {
            time_min,
            time_max,
            text,
            max_events,
        };
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let events = client
                .list_events(&calendar, &query)
                .await
                .map_err(into_py_runtime_error)?;
            pythonize_owned(&events)
        })
    }

    /// Fetch one event by id.
    #[pyo3(signature = (event_id, calendar_id = None))]
    fn event<'py>(
        &self,
        py: Python<'py>,
        event_id: String,
        calendar_id: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        let calendar = calendar_id.unwrap_or_else(|| google_calendar::PRIMARY_CALENDAR.to_owned());
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let event = client
                .get_event(&calendar, &event_id)
                .await
                .map_err(into_py_runtime_error)?;
            pythonize_owned(&event)
        })
    }

    /// Create an event. `start` and `end` are RFC 3339 (timed) or
    /// `YYYY-MM-DD` (all-day; `end` is the inclusive last day). `notify`
    /// is `all` (default), `external-only`, or `none`.
    #[pyo3(signature = (
        summary,
        start,
        end,
        all_day = false,
        description = None,
        location = None,
        attendees = None,
        notify = None,
        calendar_id = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn create_event<'py>(
        &self,
        py: Python<'py>,
        summary: String,
        start: String,
        end: String,
        all_day: bool,
        description: Option<String>,
        location: Option<String>,
        attendees: Option<Vec<String>>,
        notify: Option<String>,
        calendar_id: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        let start_time = parse_event_time(&start, all_day, "start")?;
        let end_time = parse_event_end(&end, all_day)?;
        let draft = EventDraft {
            summary,
            description,
            location,
            start: start_time,
            end: end_time,
            attendees: attendees
                .unwrap_or_default()
                .into_iter()
                .map(|email| AttendeeDraft { email })
                .collect(),
        };
        let calendar = calendar_id.unwrap_or_else(|| google_calendar::PRIMARY_CALENDAR.to_owned());
        let send_updates = parse_send_updates(notify.as_deref())?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let event = client
                .create_event(&calendar, &draft, send_updates)
                .await
                .map_err(into_py_runtime_error)?;
            pythonize_owned(&event)
        })
    }

    /// Cancel (delete) an event.
    #[pyo3(signature = (event_id, calendar_id = None, notify = None))]
    fn cancel_event<'py>(
        &self,
        py: Python<'py>,
        event_id: String,
        calendar_id: Option<String>,
        notify: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = Arc::clone(&self.inner);
        let calendar = calendar_id.unwrap_or_else(|| google_calendar::PRIMARY_CALENDAR.to_owned());
        let send_updates = parse_send_updates(notify.as_deref())?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            client
                .cancel_event(&calendar, &event_id, send_updates)
                .await
                .map_err(into_py_runtime_error)?;
            Python::attach(|py| Ok(py.None()))
        })
    }
}

#[pymodule]
fn _ix_google(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<GmailClient>()?;
    module.add_class::<CalendarClient>()?;
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
