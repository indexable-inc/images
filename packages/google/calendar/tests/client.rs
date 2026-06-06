//! Wire-level tests against a local mock of the Calendar API. These defend
//! the protocol invariants a refactor could silently break: pagination,
//! query parameters, request bodies, and error mapping. The OAuth flow is
//! tested separately in the `google-auth` crate; the only auth concern
//! here is that the calendar `Error::Auth` variant wraps a `google_auth`
//! error transparently.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Form, Json, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use chrono::DateTime;
use google_calendar::{
    AttendeeDraft, Authenticator, Client, ClientSecrets, EVENTS_SCOPE, Error, EventDraft,
    EventQuery, EventTime, SendUpdates, StoredToken, TokenStore,
};
use serde_json::{Value, json};
use tempfile::TempDir;

/// Canned responses plus a recording of everything the client sent.
struct MockCalendar {
    token_status: u16,
    token_body: Value,
    /// Successive `events.list` responses, indexed by call count.
    pages: Vec<Value>,
    event_body: Value,
    create_body: Value,
    delete_status: u16,
    delete_body: Value,
    seen: Mutex<Seen>,
}

// Clone lets tests snapshot the recording in one statement instead of
// holding the mutex guard across their assertions.
#[derive(Default, Clone)]
struct Seen {
    token_forms: Vec<HashMap<String, String>>,
    list_queries: Vec<HashMap<String, String>>,
    creates: Vec<CreateCall>,
    deletes: Vec<DeleteCall>,
}

#[derive(Clone)]
struct CreateCall {
    query: HashMap<String, String>,
    body: Value,
}

#[derive(Clone)]
struct DeleteCall {
    event_id: String,
    query: HashMap<String, String>,
}

impl Default for MockCalendar {
    fn default() -> Self {
        Self {
            token_status: 200,
            token_body: json!({ "access_token": "at-1", "expires_in": 3600 }),
            pages: vec![json!({ "items": [] })],
            event_body: json!({ "id": "evt-1", "summary": "one" }),
            create_body: json!({ "id": "evt-new", "summary": "created" }),
            delete_status: 204,
            delete_body: Value::Null,
            seen: Mutex::default(),
        }
    }
}

async fn token(
    State(mock): State<Arc<MockCalendar>>,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    mock.seen.lock().unwrap().token_forms.push(form);
    let status = StatusCode::from_u16(mock.token_status).unwrap();
    (status, Json(mock.token_body.clone()))
}

async fn list(
    State(mock): State<Arc<MockCalendar>>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    let index = {
        let mut seen = mock.seen.lock().unwrap();
        seen.list_queries.push(query);
        seen.list_queries.len() - 1
    };
    Json(
        mock.pages
            .get(index)
            .cloned()
            .unwrap_or_else(|| json!({ "items": [] })),
    )
}

async fn create(
    State(mock): State<Arc<MockCalendar>>,
    Query(query): Query<HashMap<String, String>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    mock.seen
        .lock()
        .unwrap()
        .creates
        .push(CreateCall { query, body });
    Json(mock.create_body.clone())
}

async fn get_one(State(mock): State<Arc<MockCalendar>>) -> Json<Value> {
    Json(mock.event_body.clone())
}

async fn delete_one(
    State(mock): State<Arc<MockCalendar>>,
    Path((_calendar, event_id)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    mock.seen
        .lock()
        .unwrap()
        .deletes
        .push(DeleteCall { event_id, query });
    let status = StatusCode::from_u16(mock.delete_status).unwrap();
    (status, Json(mock.delete_body.clone()))
}

/// Serve the mock on an ephemeral loopback port and return its base URL.
async fn serve(mock: Arc<MockCalendar>) -> String {
    let app = Router::new()
        .route("/token", post(token))
        .route("/calendars/{calendar}/events", get(list).post(create))
        .route(
            "/calendars/{calendar}/events/{event}",
            get(get_one).delete(delete_one),
        )
        .with_state(mock);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn test_secrets() -> ClientSecrets {
    ClientSecrets {
        client_id: "client-1.apps.googleusercontent.com".to_owned(),
        client_secret: "shhh".to_owned(),
    }
}

/// A store in `dir` seeded with a refresh token, as `gmail auth` (or
/// `gcal auth`) leaves it.
fn seeded_store(dir: &TempDir) -> TokenStore {
    let store = TokenStore::at(dir.path().join("token.json"));
    store
        .save(&StoredToken {
            refresh_token: "1//refresh".to_owned(),
            scopes: vec![EVENTS_SCOPE.to_owned()],
        })
        .unwrap();
    store
}

fn client_against(base: &str, store: TokenStore) -> Client {
    let auth = Authenticator::new(test_secrets(), store, &[EVENTS_SCOPE])
        .unwrap()
        .with_token_endpoint(format!("{base}/token"));
    Client::with_base_url(auth, base).unwrap()
}

#[tokio::test]
async fn list_paginates_and_carries_the_window() {
    let mock = Arc::new(MockCalendar {
        pages: vec![
            json!({
                "items": [{ "id": "a" }, { "id": "b" }],
                "nextPageToken": "page-2",
            }),
            json!({ "items": [{ "id": "c" }, { "id": "d" }] }),
        ],
        ..MockCalendar::default()
    });
    let base = serve(Arc::clone(&mock)).await;
    let dir = TempDir::new().unwrap();
    let client = client_against(&base, seeded_store(&dir));

    let query = EventQuery {
        time_min: Some(DateTime::parse_from_rfc3339("2026-06-05T00:00:00+02:00").unwrap()),
        time_max: Some(DateTime::parse_from_rfc3339("2026-06-12T00:00:00+02:00").unwrap()),
        text: Some("standup".to_owned()),
        max_events: 3,
    };
    let events = client.list_events("primary", &query).await.unwrap();

    let ids: Vec<&str> = events.iter().map(|event| event.id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"], "pagination must stop at max_events");

    let seen = mock.seen.lock().unwrap().clone();
    let first = &seen.list_queries[0];
    assert_eq!(first["singleEvents"], "true");
    assert_eq!(first["orderBy"], "startTime");
    assert_eq!(first["maxResults"], "3");
    assert_eq!(first["timeMin"], "2026-06-05T00:00:00+02:00");
    assert_eq!(first["timeMax"], "2026-06-12T00:00:00+02:00");
    assert_eq!(first["q"], "standup");
    let second = &seen.list_queries[1];
    assert_eq!(second["pageToken"], "page-2");
    assert_eq!(
        second["maxResults"], "1",
        "second page asks only for the remainder"
    );
    assert_eq!(seen.token_forms.len(), 1, "one refresh covers both pages");
    assert_eq!(seen.token_forms[0]["grant_type"], "refresh_token");
    assert_eq!(seen.token_forms[0]["refresh_token"], "1//refresh");
}

#[tokio::test]
async fn create_posts_the_draft_and_the_notification_policy() {
    let mock = Arc::new(MockCalendar::default());
    let base = serve(Arc::clone(&mock)).await;
    let dir = TempDir::new().unwrap();
    let client = client_against(&base, seeded_store(&dir));

    let draft = EventDraft {
        summary: "Design review".to_owned(),
        description: None,
        location: Some("Room 2".to_owned()),
        start: EventTime::Timed {
            date_time: DateTime::parse_from_rfc3339("2026-06-05T09:30:00-07:00").unwrap(),
            time_zone: None,
        },
        end: EventTime::Timed {
            date_time: DateTime::parse_from_rfc3339("2026-06-05T10:00:00-07:00").unwrap(),
            time_zone: None,
        },
        attendees: vec![AttendeeDraft {
            email: "a@example.com".to_owned(),
        }],
    };
    let created = client
        .create_event("primary", &draft, SendUpdates::All)
        .await
        .unwrap();
    assert_eq!(created.id, "evt-new");

    let seen = mock.seen.lock().unwrap().clone();
    let call = &seen.creates[0];
    assert_eq!(call.query["sendUpdates"], "all");
    assert_eq!(
        call.body,
        json!({
            "summary": "Design review",
            "location": "Room 2",
            "start": { "dateTime": "2026-06-05T09:30:00-07:00" },
            "end": { "dateTime": "2026-06-05T10:00:00-07:00" },
            "attendees": [{ "email": "a@example.com" }],
        }),
        "the draft must serialize to exactly the wire shape",
    );
}

#[tokio::test]
async fn all_day_drafts_serialize_as_dates() {
    let mock = Arc::new(MockCalendar::default());
    let base = serve(Arc::clone(&mock)).await;
    let dir = TempDir::new().unwrap();
    let client = client_against(&base, seeded_store(&dir));

    let draft = EventDraft {
        summary: "Offsite".to_owned(),
        description: None,
        location: None,
        start: EventTime::AllDay {
            date: "2026-06-10".parse().unwrap(),
        },
        end: EventTime::AllDay {
            date: "2026-06-13".parse().unwrap(),
        },
        attendees: Vec::new(),
    };
    client
        .create_event("primary", &draft, SendUpdates::None)
        .await
        .unwrap();

    let seen = mock.seen.lock().unwrap().clone();
    let call = &seen.creates[0];
    assert_eq!(call.query["sendUpdates"], "none");
    assert_eq!(
        call.body,
        json!({
            "summary": "Offsite",
            "start": { "date": "2026-06-10" },
            "end": { "date": "2026-06-13" },
        }),
    );
}

#[tokio::test]
async fn get_event_fetches_one_event_by_id() {
    let mock = Arc::new(MockCalendar {
        event_body: json!({ "id": "evt-7", "summary": "1:1", "status": "confirmed" }),
        ..MockCalendar::default()
    });
    let base = serve(Arc::clone(&mock)).await;
    let dir = TempDir::new().unwrap();
    let client = client_against(&base, seeded_store(&dir));

    let event = client.get_event("primary", "evt-7").await.unwrap();
    assert_eq!(event.id, "evt-7");
    assert_eq!(event.summary.as_deref(), Some("1:1"));
}

#[tokio::test]
async fn cancel_deletes_with_the_notification_policy() {
    let mock = Arc::new(MockCalendar::default());
    let base = serve(Arc::clone(&mock)).await;
    let dir = TempDir::new().unwrap();
    let client = client_against(&base, seeded_store(&dir));

    client
        .cancel_event("team@example.com", "evt-9", SendUpdates::ExternalOnly)
        .await
        .unwrap();

    let seen = mock.seen.lock().unwrap().clone();
    assert_eq!(seen.deletes[0].event_id, "evt-9");
    assert_eq!(seen.deletes[0].query["sendUpdates"], "externalOnly");
}

#[tokio::test]
async fn api_errors_surface_status_and_google_message() {
    let mock = Arc::new(MockCalendar {
        delete_status: 410,
        delete_body: json!({ "error": { "code": 410, "message": "Resource has been deleted" } }),
        ..MockCalendar::default()
    });
    let base = serve(Arc::clone(&mock)).await;
    let dir = TempDir::new().unwrap();
    let client = client_against(&base, seeded_store(&dir));

    let err = client
        .cancel_event("primary", "evt-9", SendUpdates::None)
        .await
        .unwrap_err();
    let Error::Api { status, message } = &err else {
        panic!("expected Error::Api, got {err:?}");
    };
    assert_eq!(*status, 410);
    assert_eq!(message, "Resource has been deleted");
}

#[tokio::test]
async fn auth_errors_wrap_through_transparently() {
    // The OAuth flow lives in google-auth and is tested there. This test
    // proves the calendar Error::Auth wrapping carries the typed inner
    // variant through, so a caller pattern-matching on the source can act
    // on it without parsing strings.
    let mock = Arc::new(MockCalendar {
        token_status: 400,
        token_body: json!({ "error": "invalid_grant" }),
        ..MockCalendar::default()
    });
    let base = serve(Arc::clone(&mock)).await;
    let dir = TempDir::new().unwrap();
    let client = client_against(&base, seeded_store(&dir));

    let query = EventQuery {
        time_min: None,
        time_max: None,
        text: None,
        max_events: 1,
    };
    let err = client.list_events("primary", &query).await.unwrap_err();
    let Error::Auth { source } = err else {
        panic!("expected Error::Auth wrapping google_auth::Error");
    };
    assert!(
        matches!(source, google_auth::Error::TokenRevoked),
        "got {source:?}"
    );
}

#[tokio::test]
async fn a_rotated_refresh_token_is_persisted() {
    let mock = Arc::new(MockCalendar {
        token_body: json!({
            "access_token": "at-1",
            "refresh_token": "1//rotated",
            "expires_in": 3600,
        }),
        ..MockCalendar::default()
    });
    let base = serve(Arc::clone(&mock)).await;
    let dir = TempDir::new().unwrap();
    let store = seeded_store(&dir);
    let client = client_against(&base, store.clone());

    let query = EventQuery {
        time_min: None,
        time_max: None,
        text: None,
        max_events: 1,
    };
    client.list_events("primary", &query).await.unwrap();

    assert_eq!(
        store.load().unwrap().refresh_token,
        "1//rotated",
        "a rotated refresh token must replace the stored one immediately",
    );
}
