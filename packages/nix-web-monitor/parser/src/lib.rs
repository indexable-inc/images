#![allow(clippy::module_name_repetitions)]

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use snafu::Snafu;

const NIX_JSON_PREFIX: &str = "@nix ";

/// Nix log level for `error`-class messages emitted via `msg` actions.
const NIX_LEVEL_ERROR: i64 = 0;

/// Maximum log entries shipped per snapshot. The UI only renders the tail and
/// we re-broadcast on every line, so the full backlog would make total bytes
/// scale O(n^2) with build verbosity.
const SNAPSHOT_LOG_LIMIT: usize = 500;

/// Maximum log entries retained server-side. Older entries get dropped from
/// the head once we exceed this; without the cap a long-lived monitor would
/// hold every line the wrapped Nix process emitted in memory forever.
const STATE_LOG_RETAIN: usize = 5_000;

/// Same idea for `messages` and `errors`; these grow once per `msg` event,
/// which is bounded but unfriendly for warning-heavy evals or long fetches.
const STATE_MESSAGE_RETAIN: usize = 2_000;

mod result_code {
    pub const FILE_LINKED: u64 = 100;
    pub const BUILD_LOG_LINE: u64 = 101;
    pub const SET_PHASE: u64 = 104;
    pub const PROGRESS: u64 = 105;
    pub const SET_EXPECTED: u64 = 106;
    pub const POST_BUILD_LOG_LINE: u64 = 107;
    pub const FETCH_STATUS: u64 = 108;
}

mod activity_code {
    pub const BUILD: u64 = 105;
}

#[derive(Debug, Snafu)]
pub enum ParseError {
    #[snafu(display("missing action field"))]
    MissingAction,

    #[snafu(display("missing numeric field {key}"))]
    MissingNumericField { key: String },

    #[snafu(display("field {key} must be an unsigned integer"))]
    NotUnsignedInteger { key: String },

    #[snafu(display("field {key} must be an integer"))]
    NotInteger { key: String },

    #[snafu(display("field {key} must be a string"))]
    NotString { key: String },

    #[snafu(display("expected one text field"))]
    OneTextField,

    #[snafu(display("expected two numeric fields"))]
    TwoNumericFields,

    #[snafu(display("expected four numeric progress fields"))]
    ProgressFields,

    #[snafu(display("expected activity type must be non-negative"))]
    NegativeActivityType,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ParsedLine {
    Event(NixEvent),
    Plain { text: String },
    ParseError { text: String, error: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum NixEvent {
    Start(StartAction),
    Stop(StopAction),
    Result(ResultAction),
    Message(MessageAction),
    Unknown { raw: Value },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartAction {
    pub id: u64,
    pub parent: Option<u64>,
    pub level: Option<i64>,
    pub text: String,
    pub activity_type: ActivityType,
    pub fields: Vec<FieldValue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopAction {
    pub id: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageAction {
    pub level: Option<i64>,
    pub message: String,
    pub raw_message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultAction {
    pub id: u64,
    pub result: ActivityResult,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityType {
    pub code: u64,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum FieldValue {
    Text(String),
    Number(i64),
    Bool(bool),
    Null,
    Other(Value),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ActivityResult {
    FileLinked {
        linked: i64,
        total: i64,
    },
    BuildLogLine {
        line: String,
    },
    SetPhase {
        phase: String,
    },
    Progress {
        progress: ActivityProgress,
    },
    SetExpected {
        activity_type: ActivityType,
        expected: i64,
    },
    PostBuildLogLine {
        line: String,
    },
    FetchStatus {
        status: String,
    },
    Other {
        result_type: u64,
        fields: Vec<FieldValue>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityProgress {
    pub done: i64,
    pub expected: i64,
    pub running: i64,
    pub failed: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorState {
    pub activities: BTreeMap<u64, ActivityNode>,
    pub builds: BTreeMap<String, BuildNode>,
    pub logs: Vec<LogEntry>,
    pub messages: Vec<String>,
    pub errors: Vec<String>,
    pub progress: Option<ActivityProgress>,
    pub expected: BTreeMap<String, i64>,
    pub exit_code: Option<i32>,
    pub finished: bool,
    /// Monotonic counter for `LogEntry.index`. Kept independent of
    /// `logs.len()` so retention pruning never reuses an index.
    log_counter: u64,
}

impl MonitorState {
    #[must_use]
    pub fn snapshot(&self) -> MonitorSnapshot {
        let log_tail_start = self.logs.len().saturating_sub(SNAPSHOT_LOG_LIMIT);
        MonitorSnapshot {
            activities: self.activities.values().cloned().collect(),
            builds: self.builds.values().cloned().collect(),
            logs: self.logs[log_tail_start..].to_vec(),
            messages: self.messages.clone(),
            errors: self.errors.clone(),
            progress: self.progress,
            expected: self.expected.clone(),
            exit_code: self.exit_code,
            finished: self.finished,
        }
    }

    pub fn apply_line(&mut self, line: &str) -> ParsedLine {
        let parsed = parse_line(line);
        self.apply_parsed_line(&parsed);
        parsed
    }

    pub fn apply_parsed_line(&mut self, parsed: &ParsedLine) {
        match parsed {
            ParsedLine::Event(event) => self.apply_event(event),
            ParsedLine::Plain { text } => self.apply_plain(text),
            ParsedLine::ParseError { text, error } => {
                self.errors
                    .push(format!("failed to parse Nix event: {error}"));
                self.push_log(None, None, text);
            }
        }
    }

    /// Settle the run and, on a clean exit, promote `Stopped` builds to
    /// `Succeeded`. Nix has no positive success marker per activity, so we
    /// wait for the process to confirm before claiming success.
    pub fn finish(&mut self, exit_code: Option<i32>) {
        self.exit_code = exit_code;
        self.finished = true;
        if exit_code == Some(0) {
            for build in self.builds.values_mut() {
                if build.status == BuildStatus::Stopped {
                    build.status = BuildStatus::Succeeded;
                }
            }
        }
    }

    fn apply_event(&mut self, event: &NixEvent) {
        match event {
            NixEvent::Start(action) => self.start_activity(action),
            NixEvent::Stop(action) => self.stop_activity(action.id),
            NixEvent::Result(action) => self.apply_result(action),
            NixEvent::Message(action) => self.apply_message(action),
            NixEvent::Unknown { .. } => {}
        }
    }

    /// Prefer Nix's clean `raw_msg`; otherwise strip ANSI ourselves. The
    /// log panel never gets literal escape bytes either way.
    fn cleaned_message(action: &MessageAction) -> String {
        action
            .raw_message
            .clone()
            .unwrap_or_else(|| strip_ansi(&action.message))
    }

    fn start_activity(&mut self, action: &StartAction) {
        let now = next_tick(self.activities.len());
        let now_ms = current_unix_ms();
        let (build, host) = if action.activity_type.code == activity_code::BUILD {
            (text_field(&action.fields, 0), text_field(&action.fields, 1))
        } else {
            (None, None)
        };

        self.activities.insert(
            action.id,
            ActivityNode {
                id: action.id,
                parent: action.parent,
                activity_type: action.activity_type.clone(),
                text: action.text.clone(),
                fields: action.fields.clone(),
                phase: None,
                progress: None,
                status: ActivityStatus::Running,
                started_tick: now,
                stopped_tick: None,
                started_at_ms: now_ms,
                stopped_at_ms: None,
                build: build.clone(),
            },
        );

        if let Some(derivation) = build {
            self.builds.insert(
                derivation.clone(),
                BuildNode {
                    derivation,
                    activity_id: Some(action.id),
                    host,
                    phase: None,
                    status: BuildStatus::Running,
                    log_count: 0,
                    started_at_ms: now_ms,
                    stopped_at_ms: None,
                },
            );
        }
    }

    /// Mark the activity stopped. The build status moves to `Stopped` and
    /// stays there until either a builder failure arrives (`Failed`) or the
    /// process exits cleanly (`finish` promotes to `Succeeded`). Nix never
    /// emits a per-activity success signal, so we cannot do better without
    /// inventing one.
    fn stop_activity(&mut self, id: u64) {
        let now_ms = current_unix_ms();
        if let Some(activity) = self.activities.get_mut(&id) {
            activity.status = ActivityStatus::Stopped;
            activity.stopped_tick = Some(next_tick(self.logs.len()));
            activity.stopped_at_ms = Some(now_ms);
            if let Some(build) = &activity.build
                && let Some(build_node) = self.builds.get_mut(build)
                && build_node.status == BuildStatus::Running
            {
                build_node.status = BuildStatus::Stopped;
                build_node.stopped_at_ms = Some(now_ms);
            }
        }
    }

    fn apply_result(&mut self, action: &ResultAction) {
        match &action.result {
            ActivityResult::BuildLogLine { line } | ActivityResult::PostBuildLogLine { line } => {
                let cleaned = strip_ansi(line);
                self.push_log(Some(action.id), None, &cleaned);
            }
            ActivityResult::SetPhase { phase } => {
                if let Some(activity) = self.activities.get_mut(&action.id) {
                    activity.phase = Some(phase.clone());
                    if let Some(build) = &activity.build
                        && let Some(build_node) = self.builds.get_mut(build)
                    {
                        build_node.phase = Some(phase.clone());
                    }
                }
            }
            ActivityResult::Progress { progress } => {
                self.progress = Some(*progress);
                if let Some(activity) = self.activities.get_mut(&action.id) {
                    activity.progress = Some(*progress);
                }
            }
            ActivityResult::SetExpected {
                activity_type,
                expected,
            } => {
                self.expected.insert(activity_type.name.clone(), *expected);
            }
            ActivityResult::FetchStatus { status } => {
                self.messages.push(status.clone());
            }
            ActivityResult::FileLinked { .. } | ActivityResult::Other { .. } => {}
        }
    }

    fn apply_message(&mut self, action: &MessageAction) {
        let cleaned = Self::cleaned_message(action);
        self.messages.push(cleaned.clone());
        truncate_head(&mut self.messages, STATE_MESSAGE_RETAIN);
        if action.level == Some(NIX_LEVEL_ERROR) {
            self.errors.push(cleaned.clone());
            truncate_head(&mut self.errors, STATE_MESSAGE_RETAIN);
        }
        // Surface operator messages in the UI log panel too; otherwise an
        // eval failure shows up as an empty log with only an exit code.
        self.push_log(None, action.level, &cleaned);

        // Failure detection runs on the formatted msg (which preserves the
        // "error: ..." prefix that `parse_builder_failure` keys on); raw_msg
        // is the body of the message without the severity word so the
        // cleaned form would silently miss modern Nix failure patterns.
        let stripped = strip_ansi(&action.message);
        if let Some(failure) = parse_builder_failure(&stripped) {
            self.mark_failed_build(failure);
        }
    }

    fn apply_plain(&mut self, text: &str) {
        let stripped = strip_ansi(text);
        if let Some(failure) = parse_builder_failure(&stripped) {
            self.mark_failed_build(failure);
        }
        self.push_log(None, None, &stripped);
    }

    fn mark_failed_build(&mut self, failure: BuilderFailure) {
        use std::collections::btree_map::Entry;
        let now_ms = current_unix_ms();
        match self.builds.entry(failure.derivation.clone()) {
            Entry::Occupied(mut entry) => {
                let build = entry.get_mut();
                build.status = BuildStatus::Failed;
                if build.stopped_at_ms.is_none() {
                    build.stopped_at_ms = Some(now_ms);
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(BuildNode {
                    derivation: failure.derivation,
                    activity_id: None,
                    host: None,
                    phase: None,
                    status: BuildStatus::Failed,
                    log_count: 0,
                    started_at_ms: now_ms,
                    stopped_at_ms: Some(now_ms),
                });
            }
        }
    }

    fn push_log(&mut self, activity_id: Option<u64>, level: Option<i64>, text: &str) {
        let index = usize::try_from(self.log_counter).unwrap_or(usize::MAX);
        self.log_counter = self.log_counter.saturating_add(1);
        if let Some(id) = activity_id
            && let Some(activity) = self.activities.get(&id)
            && let Some(build) = &activity.build
            && let Some(build_node) = self.builds.get_mut(build)
        {
            build_node.log_count += 1;
        }

        self.logs.push(LogEntry {
            index,
            activity_id,
            level,
            text: text.to_owned(),
        });
        truncate_head(&mut self.logs, STATE_LOG_RETAIN);
    }
}

/// Drop entries from the front so the collection never exceeds `max`.
/// Cheap when `max - new.len()` is small (the common steady-state).
fn truncate_head<T>(items: &mut Vec<T>, max: usize) {
    if items.len() > max {
        items.drain(..items.len() - max);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorSnapshot {
    pub activities: Vec<ActivityNode>,
    pub builds: Vec<BuildNode>,
    pub logs: Vec<LogEntry>,
    pub messages: Vec<String>,
    pub errors: Vec<String>,
    pub progress: Option<ActivityProgress>,
    pub expected: BTreeMap<String, i64>,
    pub exit_code: Option<i32>,
    pub finished: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityNode {
    pub id: u64,
    pub parent: Option<u64>,
    pub activity_type: ActivityType,
    pub text: String,
    pub fields: Vec<FieldValue>,
    pub phase: Option<String>,
    pub progress: Option<ActivityProgress>,
    pub status: ActivityStatus,
    pub started_tick: u64,
    pub stopped_tick: Option<u64>,
    /// Unix epoch milliseconds when the activity started, stamped by the
    /// parser at apply time. Lets the UI render live durations without
    /// needing the original event timestamps from Nix.
    pub started_at_ms: u64,
    pub stopped_at_ms: Option<u64>,
    pub build: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatus {
    Running,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildNode {
    pub derivation: String,
    pub activity_id: Option<u64>,
    pub host: Option<String>,
    pub phase: Option<String>,
    pub status: BuildStatus,
    pub log_count: usize,
    pub started_at_ms: u64,
    pub stopped_at_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildStatus {
    Running,
    /// Activity finished without an error reference; outcome unknown until
    /// the wrapping process exits.
    Stopped,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub index: usize,
    pub activity_id: Option<u64>,
    /// Nix log level when known (0=error, 1=warn, 2=notice, 3=info, ...).
    /// `None` for builder output and plain stdout lines.
    pub level: Option<i64>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BuilderFailure {
    derivation: String,
}

#[must_use]
pub fn parse_line(line: &str) -> ParsedLine {
    let Some(raw_json) = line.strip_prefix(NIX_JSON_PREFIX) else {
        return ParsedLine::Plain {
            text: line.to_owned(),
        };
    };

    match serde_json::from_str::<Value>(raw_json) {
        Ok(raw) => match parse_event(raw) {
            Ok(event) => ParsedLine::Event(event),
            Err(error) => ParsedLine::ParseError {
                text: line.to_owned(),
                error: error.to_string(),
            },
        },
        Err(error) => ParsedLine::ParseError {
            text: line.to_owned(),
            error: error.to_string(),
        },
    }
}

fn parse_event(raw: Value) -> Result<NixEvent, ParseError> {
    let action = raw
        .get("action")
        .and_then(Value::as_str)
        .ok_or(ParseError::MissingAction)?;

    match action {
        "start" => parse_start(&raw).map(NixEvent::Start),
        "stop" => parse_stop(&raw).map(NixEvent::Stop),
        "result" => parse_result(&raw).map(NixEvent::Result),
        "msg" => parse_message(&raw).map(NixEvent::Message),
        _ => Ok(NixEvent::Unknown { raw }),
    }
}

fn parse_start(raw: &Value) -> Result<StartAction, ParseError> {
    let activity_type = activity_type_for(required_u64(raw, "type")?);
    Ok(StartAction {
        id: required_u64(raw, "id")?,
        parent: optional_u64(raw, "parent")?,
        level: optional_i64(raw, "level")?,
        text: optional_string(raw, "text")?.unwrap_or_default(),
        activity_type,
        fields: fields(raw),
    })
}

fn parse_stop(raw: &Value) -> Result<StopAction, ParseError> {
    Ok(StopAction {
        id: required_u64(raw, "id")?,
    })
}

fn parse_message(raw: &Value) -> Result<MessageAction, ParseError> {
    Ok(MessageAction {
        level: optional_i64(raw, "level")?,
        message: optional_string(raw, "msg")?.unwrap_or_default(),
        raw_message: optional_string(raw, "raw_msg")?,
    })
}

fn parse_result(raw: &Value) -> Result<ResultAction, ParseError> {
    let result_type = required_u64(raw, "type")?;
    let fields = fields(raw);
    let result = match result_type {
        result_code::FILE_LINKED => {
            let (linked, total) = two_numbers(&fields)?;
            ActivityResult::FileLinked { linked, total }
        }
        result_code::BUILD_LOG_LINE => ActivityResult::BuildLogLine {
            line: one_text(&fields)?,
        },
        result_code::SET_PHASE => ActivityResult::SetPhase {
            phase: one_text(&fields)?,
        },
        result_code::PROGRESS => ActivityResult::Progress {
            progress: parse_progress(&fields)?,
        },
        result_code::SET_EXPECTED => {
            let (activity_type_code, expected) = two_numbers(&fields)?;
            let activity_type_code = u64::try_from(activity_type_code)
                .map_err(|_| ParseError::NegativeActivityType)?;
            ActivityResult::SetExpected {
                activity_type: activity_type_for(activity_type_code),
                expected,
            }
        }
        result_code::POST_BUILD_LOG_LINE => ActivityResult::PostBuildLogLine {
            line: one_text(&fields)?,
        },
        result_code::FETCH_STATUS => ActivityResult::FetchStatus {
            status: one_text(&fields)?,
        },
        _ => ActivityResult::Other {
            result_type,
            fields,
        },
    };

    Ok(ResultAction {
        id: required_u64(raw, "id")?,
        result,
    })
}

fn fields(raw: &Value) -> Vec<FieldValue> {
    raw.get("fields")
        .and_then(Value::as_array)
        .map(|values| values.iter().map(field_value).collect())
        .unwrap_or_default()
}

fn field_value(value: &Value) -> FieldValue {
    match value {
        Value::String(text) => FieldValue::Text(text.clone()),
        Value::Number(number) => number
            .as_i64()
            .map(FieldValue::Number)
            .unwrap_or_else(|| FieldValue::Other(value.clone())),
        Value::Bool(value) => FieldValue::Bool(*value),
        Value::Null => FieldValue::Null,
        Value::Array(_) | Value::Object(_) => FieldValue::Other(value.clone()),
    }
}

fn one_text(fields: &[FieldValue]) -> Result<String, ParseError> {
    match fields {
        [FieldValue::Text(text)] => Ok(text.clone()),
        _ => Err(ParseError::OneTextField),
    }
}

fn two_numbers(fields: &[FieldValue]) -> Result<(i64, i64), ParseError> {
    match fields {
        [FieldValue::Number(first), FieldValue::Number(second)] => Ok((*first, *second)),
        _ => Err(ParseError::TwoNumericFields),
    }
}

fn parse_progress(fields: &[FieldValue]) -> Result<ActivityProgress, ParseError> {
    match fields {
        [
            FieldValue::Number(done),
            FieldValue::Number(expected),
            FieldValue::Number(running),
            FieldValue::Number(failed),
        ] => Ok(ActivityProgress {
            done: *done,
            expected: *expected,
            running: *running,
            failed: *failed,
        }),
        _ => Err(ParseError::ProgressFields),
    }
}

fn required_u64(raw: &Value, key: &str) -> Result<u64, ParseError> {
    raw.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| ParseError::MissingNumericField {
            key: key.to_owned(),
        })
}

fn optional_u64(raw: &Value, key: &str) -> Result<Option<u64>, ParseError> {
    match raw.get(key) {
        Some(Value::Null) | None => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| ParseError::NotUnsignedInteger {
                key: key.to_owned(),
            }),
    }
}

fn optional_i64(raw: &Value, key: &str) -> Result<Option<i64>, ParseError> {
    match raw.get(key) {
        Some(Value::Null) | None => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| ParseError::NotInteger {
                key: key.to_owned(),
            }),
    }
}

fn optional_string(raw: &Value, key: &str) -> Result<Option<String>, ParseError> {
    match raw.get(key) {
        Some(Value::Null) | None => Ok(None),
        Some(value) => value
            .as_str()
            .map(ToOwned::to_owned)
            .map(Some)
            .ok_or_else(|| ParseError::NotString {
                key: key.to_owned(),
            }),
    }
}

fn activity_type_for(code: u64) -> ActivityType {
    let name = match code {
        0 => "unknown",
        100 => "copy_path",
        101 => "file_transfer",
        102 => "realise",
        103 => "copy_paths",
        104 => "builds",
        activity_code::BUILD => "build",
        106 => "optimise_store",
        107 => "verify_paths",
        108 => "substitute",
        109 => "query_path_info",
        110 => "post_build_hook",
        111 => "build_waiting",
        112 => "fetch_tree",
        _ => "future",
    };

    ActivityType {
        code,
        name: name.to_owned(),
    }
}

fn text_field(fields: &[FieldValue], index: usize) -> Option<String> {
    fields.get(index).and_then(|field| match field {
        FieldValue::Text(value) => Some(value.clone()),
        FieldValue::Number(_) | FieldValue::Bool(_) | FieldValue::Null | FieldValue::Other(_) => {
            None
        }
    })
}

fn next_tick(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// Strip both ESC-prefixed CSI sequences and the bare `[<n>;<n>m` form that
/// shows up when an upstream encoder drops the leading `0x1B`. Used by the
/// state machine before display, by failure-message detection, and re-exported
/// for the wrapper binary's terminal renderer.
#[must_use]
pub fn strip_ansi(text: &str) -> String {
    let no_esc =
        String::from_utf8(strip_ansi_escapes::strip(text)).unwrap_or_else(|_| text.to_owned());
    strip_orphan_sgr(&no_esc)
}

/// Drop bare CSI SGR sequences like `[35;1m` / `[0m` that survive when the
/// upstream encoder loses the `ESC` byte (some Nix paths emit msg fields
/// where the leading 0x1B has been pre-stripped). The pattern is narrow on
/// purpose: digits separated by `;` terminated by `m`, so legitimate text
/// such as `[1]` or `[main]` stays intact.
fn strip_orphan_sgr(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if ch != '[' {
            out.push(ch);
            continue;
        }

        let mut end = idx + 1;
        let mut saw_digit = false;
        let mut matched = false;
        while let Some(&(j, next)) = chars.peek() {
            if next.is_ascii_digit() {
                saw_digit = true;
                end = j + 1;
                chars.next();
            } else if next == ';' && saw_digit {
                end = j + 1;
                chars.next();
            } else if next == 'm' && saw_digit {
                end = j + 1;
                chars.next();
                matched = true;
                break;
            } else {
                break;
            }
        }

        if matched {
            continue;
        }
        out.push('[');
        out.push_str(&text[idx + 1..end]);
    }
    out
}

fn parse_builder_failure(text: &str) -> Option<BuilderFailure> {
    // Legacy Nix: `error: builder for '/nix/store/...drv' failed with exit code 1`
    if let Some(after) = text.strip_prefix("error: builder for '")
        && let Some((derivation, _)) = after.split_once("' failed with exit code ")
        && derivation.ends_with(".drv")
    {
        return Some(BuilderFailure {
            derivation: derivation.to_owned(),
        });
    }

    // Modern Nix (>= 2.21): `error: Cannot build '/nix/store/...drv'. Reason: builder failed ...`
    if let Some(after) = text.strip_prefix("error: Cannot build '")
        && let Some((derivation, _)) = after.split_once("'.")
        && derivation.ends_with(".drv")
    {
        return Some(BuilderFailure {
            derivation: derivation.to_owned(),
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_build_start() {
        let line = r#"@nix {"action":"start","fields":["/nix/store/abc-demo.drv","",1,1],"id":7,"level":3,"text":"building '/nix/store/abc-demo.drv'","type":105}"#;
        let parsed = parse_line(line);

        assert!(matches!(parsed, ParsedLine::Event(NixEvent::Start(_))));
    }

    #[test]
    fn applies_build_log_and_phase() {
        let mut state = MonitorState::default();
        state.apply_line(r#"@nix {"action":"start","fields":["/nix/store/abc-demo.drv","local",1,1],"id":7,"level":3,"text":"building '/nix/store/abc-demo.drv'","type":105}"#);
        state.apply_line(r#"@nix {"action":"result","fields":["buildPhase"],"id":7,"type":104}"#);
        state.apply_line(r#"@nix {"action":"result","fields":["compiling"],"id":7,"type":101}"#);

        let snapshot = state.snapshot();
        assert_eq!(snapshot.builds[0].phase.as_deref(), Some("buildPhase"));
        assert_eq!(snapshot.builds[0].log_count, 1);
        assert_eq!(snapshot.logs[0].text, "compiling");
    }

    #[test]
    fn marks_failed_build_from_error_message() {
        let mut state = MonitorState::default();
        state.apply_line(r#"@nix {"action":"start","fields":["/nix/store/abc-demo.drv","local",1,1],"id":7,"level":3,"text":"building '/nix/store/abc-demo.drv'","type":105}"#);
        state.apply_line("error: builder for '/nix/store/abc-demo.drv' failed with exit code 1");

        assert_eq!(
            state.snapshot().builds[0].status,
            BuildStatus::Failed,
            "plain terminal messages should update failed build state"
        );
    }

    #[test]
    fn stop_then_clean_finish_promotes_to_succeeded() {
        let mut state = MonitorState::default();
        state.apply_line(r#"@nix {"action":"start","fields":["/nix/store/abc-demo.drv","local",1,1],"id":7,"level":3,"text":"building '/nix/store/abc-demo.drv'","type":105}"#);
        state.apply_line(r#"@nix {"action":"stop","id":7}"#);

        assert_eq!(
            state.snapshot().builds[0].status,
            BuildStatus::Stopped,
            "stop alone is not evidence of success"
        );

        state.finish(Some(0));
        assert_eq!(state.snapshot().builds[0].status, BuildStatus::Succeeded);
    }

    #[test]
    fn stop_then_late_error_resolves_to_failed_without_succeeded_flicker() {
        let mut state = MonitorState::default();
        state.apply_line(r#"@nix {"action":"start","fields":["/nix/store/abc-demo.drv","local",1,1],"id":7,"level":3,"text":"building '/nix/store/abc-demo.drv'","type":105}"#);
        state.apply_line(r#"@nix {"action":"stop","id":7}"#);
        state.apply_line("error: builder for '/nix/store/abc-demo.drv' failed with exit code 1");

        assert_eq!(state.snapshot().builds[0].status, BuildStatus::Failed);
    }

    #[test]
    fn error_level_message_is_recorded_as_error() {
        let mut state = MonitorState::default();
        state.apply_line(
            r#"@nix {"action":"msg","level":0,"msg":"something went wrong"}"#,
        );
        assert_eq!(state.snapshot().errors, vec!["something went wrong"]);
    }

    #[test]
    fn operator_messages_reach_the_log_stream() {
        let mut state = MonitorState::default();
        state.apply_line(
            r#"@nix {"action":"msg","level":0,"msg":"eval failed: undefined variable"}"#,
        );
        let snapshot = state.snapshot();
        assert_eq!(
            snapshot.logs.last().map(|entry| entry.text.as_str()),
            Some("eval failed: undefined variable"),
            "messages should be visible in the log panel"
        );
        assert_eq!(snapshot.logs.last().and_then(|entry| entry.level), Some(0));
    }

    #[test]
    fn ansi_codes_are_stripped_before_reaching_the_log_panel() {
        let mut state = MonitorState::default();
        // Nix emits SGR codes in `msg`; the UI never gets the raw bytes.
        state.apply_line(
            r#"@nix {"action":"msg","level":1,"msg":"[35;1mwarning:[0m unknown setting 'foo'"}"#,
        );
        let snapshot = state.snapshot();
        assert_eq!(
            snapshot.logs.last().map(|entry| entry.text.as_str()),
            Some("warning: unknown setting 'foo'")
        );
    }

    #[test]
    fn raw_msg_is_preferred_when_present() {
        let mut state = MonitorState::default();
        state.apply_line(
            r#"@nix {"action":"msg","level":0,"msg":"[31merror[0m: oops","raw_msg":"error: oops"}"#,
        );
        let snapshot = state.snapshot();
        assert_eq!(
            snapshot.logs.last().map(|entry| entry.text.as_str()),
            Some("error: oops")
        );
    }

    #[test]
    fn snapshot_caps_logs_to_the_tail() {
        let mut state = MonitorState::default();
        for i in 0..(SNAPSHOT_LOG_LIMIT + 50) {
            state.apply_line(&format!("line {i}"));
        }
        let snapshot = state.snapshot();
        assert_eq!(snapshot.logs.len(), SNAPSHOT_LOG_LIMIT);
        assert_eq!(snapshot.logs.first().unwrap().text, "line 50");
        assert_eq!(
            snapshot.logs.last().unwrap().text,
            format!("line {}", SNAPSHOT_LOG_LIMIT + 49)
        );
    }

    #[test]
    fn modern_nix_failure_message_marks_build_failed() {
        let mut state = MonitorState::default();
        state.apply_line(r#"@nix {"action":"start","fields":["/nix/store/xyz-demo.drv","local",1,1],"id":9,"level":3,"text":"building '/nix/store/xyz-demo.drv'","type":105}"#);
        state.apply_line(r#"@nix {"action":"stop","id":9}"#);
        state.apply_line(
            "error: Cannot build '/nix/store/xyz-demo.drv'. Reason: builder failed with exit code 1.",
        );
        assert_eq!(state.snapshot().builds[0].status, BuildStatus::Failed);
    }

    #[test]
    fn raw_msg_does_not_hide_msg_based_failure_detection() {
        let mut state = MonitorState::default();
        state.apply_line(r#"@nix {"action":"start","fields":["/nix/store/xyz-demo.drv","local",1,1],"id":11,"level":3,"text":"building '/nix/store/xyz-demo.drv'","type":105}"#);
        state.apply_line(r#"@nix {"action":"stop","id":11}"#);
        // Nix sometimes ships the formatted message in `msg` and a stripped
        // body in `raw_msg`. The display string is the raw body, but failure
        // detection still needs to key on the formatted `msg` prefix.
        state.apply_line(
            r#"@nix {"action":"msg","level":0,"msg":"error: Cannot build '/nix/store/xyz-demo.drv'. Reason: builder failed with exit code 1.","raw_msg":"Cannot build '/nix/store/xyz-demo.drv'. Reason: builder failed with exit code 1."}"#,
        );
        assert_eq!(state.snapshot().builds[0].status, BuildStatus::Failed);
    }

    #[test]
    fn log_retention_drops_head_but_keeps_indices_monotonic() {
        let mut state = MonitorState::default();
        let total = STATE_LOG_RETAIN + 50;
        for i in 0..total {
            state.apply_line(&format!("line {i}"));
        }
        assert_eq!(state.logs.len(), STATE_LOG_RETAIN);
        let first_index = state.logs.first().unwrap().index;
        let last_index = state.logs.last().unwrap().index;
        assert_eq!(first_index, 50, "head is dropped after retention overflow");
        assert_eq!(last_index, total - 1, "indices never reused");
    }

    #[test]
    fn message_retention_caps_messages_array() {
        let mut state = MonitorState::default();
        for i in 0..(STATE_MESSAGE_RETAIN + 25) {
            state.apply_line(&format!(
                "@nix {{\"action\":\"msg\",\"level\":3,\"msg\":\"warn {i}\"}}"
            ));
        }
        assert_eq!(state.snapshot().messages.len(), STATE_MESSAGE_RETAIN);
    }
}
