#![allow(clippy::module_name_repetitions)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

const NIX_JSON_PREFIX: &str = "@nix ";
const RESULT_BUILD_LOG_LINE: u64 = 101;
const RESULT_SET_PHASE: u64 = 104;
const RESULT_PROGRESS: u64 = 105;
const RESULT_SET_EXPECTED: u64 = 106;
const RESULT_POST_BUILD_LOG_LINE: u64 = 107;
const RESULT_FETCH_STATUS: u64 = 108;
const ACTIVITY_BUILD: u64 = 105;

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
}

impl MonitorState {
    #[must_use]
    pub fn snapshot(&self) -> MonitorSnapshot {
        MonitorSnapshot {
            activities: self.activities.values().cloned().collect(),
            builds: self.builds.values().cloned().collect(),
            logs: self.logs.clone(),
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
                self.push_log(None, text);
            }
        }
    }

    pub fn finish(&mut self, exit_code: Option<i32>) {
        self.exit_code = exit_code;
        self.finished = true;
    }

    fn apply_event(&mut self, event: &NixEvent) {
        match event {
            NixEvent::Start(action) => self.start_activity(action),
            NixEvent::Stop(action) => self.stop_activity(action.id),
            NixEvent::Result(action) => self.apply_result(action),
            NixEvent::Message(action) => self.apply_message(action),
            NixEvent::Unknown { raw } => self.messages.push(raw.to_string()),
        }
    }

    fn start_activity(&mut self, action: &StartAction) {
        let now = next_tick(self.activities.len());
        let build = if action.activity_type.code == ACTIVITY_BUILD {
            first_text_field(&action.fields)
        } else {
            None
        };
        let host = if action.activity_type.code == ACTIVITY_BUILD {
            text_field(&action.fields, 1)
        } else {
            None
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
                build: build.clone(),
            },
        );

        if let Some(derivation) = build {
            self.builds.insert(
                derivation.clone(),
                BuildNode {
                    derivation,
                    activity_id: action.id,
                    host,
                    phase: None,
                    status: BuildStatus::Running,
                    log_count: 0,
                },
            );
        }
    }

    fn stop_activity(&mut self, id: u64) {
        if let Some(activity) = self.activities.get_mut(&id) {
            activity.status = ActivityStatus::Stopped;
            activity.stopped_tick = Some(next_tick(self.logs.len()));
            if let Some(build) = &activity.build
                && let Some(build_node) = self.builds.get_mut(build)
                && build_node.status == BuildStatus::Running
            {
                build_node.status = BuildStatus::Succeeded;
            }
        }
    }

    fn apply_result(&mut self, action: &ResultAction) {
        match &action.result {
            ActivityResult::BuildLogLine { line } | ActivityResult::PostBuildLogLine { line } => {
                self.push_log(Some(action.id), line);
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
        self.messages.push(action.message.clone());
        let stripped = strip_ansi(&action.message);
        if stripped.starts_with("error:") {
            self.errors.push(action.message.clone());
        }

        if let Some(failure) = parse_builder_failure(&stripped) {
            self.mark_failed_build(failure);
        }
    }

    fn apply_plain(&mut self, text: &str) {
        let stripped = strip_ansi(text);
        if let Some(failure) = parse_builder_failure(&stripped) {
            self.mark_failed_build(failure);
        }
        self.push_log(None, text);
    }

    fn mark_failed_build(&mut self, failure: BuilderFailure) {
        if let Some(build) = self.builds.get_mut(&failure.derivation) {
            build.status = BuildStatus::Failed;
        } else {
            self.builds.insert(
                failure.derivation.clone(),
                BuildNode {
                    derivation: failure.derivation,
                    activity_id: 0,
                    host: None,
                    phase: None,
                    status: BuildStatus::Failed,
                    log_count: 0,
                },
            );
        }
    }

    fn push_log(&mut self, activity_id: Option<u64>, text: &str) {
        let index = self.logs.len();
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
            text: text.to_owned(),
        });
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
    pub activity_id: u64,
    pub host: Option<String>,
    pub phase: Option<String>,
    pub status: BuildStatus,
    pub log_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildStatus {
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub index: usize,
    pub activity_id: Option<u64>,
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
                error,
            },
        },
        Err(error) => ParsedLine::ParseError {
            text: line.to_owned(),
            error: error.to_string(),
        },
    }
}

fn parse_event(raw: Value) -> Result<NixEvent, String> {
    let action = raw
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing action field".to_owned())?;

    match action {
        "start" => parse_start(&raw).map(NixEvent::Start),
        "stop" => parse_stop(&raw).map(NixEvent::Stop),
        "result" => parse_result(&raw).map(NixEvent::Result),
        "msg" => parse_message(&raw).map(NixEvent::Message),
        _ => Ok(NixEvent::Unknown { raw }),
    }
}

fn parse_start(raw: &Value) -> Result<StartAction, String> {
    let activity_type = activity_type(required_u64(raw, "type")?);
    Ok(StartAction {
        id: required_u64(raw, "id")?,
        parent: optional_u64(raw, "parent")?,
        level: optional_i64(raw, "level")?,
        text: optional_string(raw, "text")?.unwrap_or_default(),
        activity_type,
        fields: fields(raw),
    })
}

fn parse_stop(raw: &Value) -> Result<StopAction, String> {
    Ok(StopAction {
        id: required_u64(raw, "id")?,
    })
}

fn parse_message(raw: &Value) -> Result<MessageAction, String> {
    Ok(MessageAction {
        level: optional_i64(raw, "level")?,
        message: optional_string(raw, "msg")?.unwrap_or_default(),
        raw_message: optional_string(raw, "raw_msg")?,
    })
}

fn parse_result(raw: &Value) -> Result<ResultAction, String> {
    let result_type = required_u64(raw, "type")?;
    let fields = fields(raw);
    let result = match result_type {
        100 => {
            let (linked, total) = two_numbers(&fields)?;
            ActivityResult::FileLinked { linked, total }
        }
        RESULT_BUILD_LOG_LINE => ActivityResult::BuildLogLine {
            line: one_text(&fields)?,
        },
        RESULT_SET_PHASE => ActivityResult::SetPhase {
            phase: one_text(&fields)?,
        },
        RESULT_PROGRESS => {
            let progress_fields = four_numbers(&fields)?;
            ActivityResult::Progress {
                progress: ActivityProgress {
                    done: progress_fields.done,
                    expected: progress_fields.expected,
                    running: progress_fields.running,
                    failed: progress_fields.failed,
                },
            }
        }
        RESULT_SET_EXPECTED => {
            let (activity_type_code, expected) = two_numbers(&fields)?;
            let activity_type_code = u64::try_from(activity_type_code)
                .map_err(|_| "expected activity type must be non-negative".to_owned())?;
            ActivityResult::SetExpected {
                activity_type: activity_type(activity_type_code),
                expected,
            }
        }
        RESULT_POST_BUILD_LOG_LINE => ActivityResult::PostBuildLogLine {
            line: one_text(&fields)?,
        },
        RESULT_FETCH_STATUS => ActivityResult::FetchStatus {
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

#[derive(Clone, Copy)]
struct FourNumbers {
    done: i64,
    expected: i64,
    running: i64,
    failed: i64,
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

fn one_text(fields: &[FieldValue]) -> Result<String, String> {
    match fields {
        [FieldValue::Text(text)] => Ok(text.clone()),
        _ => Err("expected one text field".to_owned()),
    }
}

fn two_numbers(fields: &[FieldValue]) -> Result<(i64, i64), String> {
    match fields {
        [FieldValue::Number(first), FieldValue::Number(second)] => Ok((*first, *second)),
        _ => Err("expected two numeric fields".to_owned()),
    }
}

fn four_numbers(fields: &[FieldValue]) -> Result<FourNumbers, String> {
    match fields {
        [
            FieldValue::Number(done),
            FieldValue::Number(expected),
            FieldValue::Number(running),
            FieldValue::Number(failed),
        ] => Ok(FourNumbers {
            done: *done,
            expected: *expected,
            running: *running,
            failed: *failed,
        }),
        _ => Err("expected four numeric fields".to_owned()),
    }
}

fn required_u64(raw: &Value, key: &str) -> Result<u64, String> {
    raw.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("missing numeric field {key}"))
}

fn optional_u64(raw: &Value, key: &str) -> Result<Option<u64>, String> {
    match raw.get(key) {
        Some(Value::Null) | None => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("field {key} must be an unsigned integer")),
    }
}

fn optional_i64(raw: &Value, key: &str) -> Result<Option<i64>, String> {
    match raw.get(key) {
        Some(Value::Null) | None => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("field {key} must be an integer")),
    }
}

fn optional_string(raw: &Value, key: &str) -> Result<Option<String>, String> {
    match raw.get(key) {
        Some(Value::Null) | None => Ok(None),
        Some(value) => value
            .as_str()
            .map(ToOwned::to_owned)
            .map(Some)
            .ok_or_else(|| format!("field {key} must be a string")),
    }
}

fn activity_type(code: u64) -> ActivityType {
    let name = match code {
        0 => "unknown",
        100 => "copy_path",
        101 => "file_transfer",
        102 => "realise",
        103 => "copy_paths",
        104 => "builds",
        ACTIVITY_BUILD => "build",
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

fn first_text_field(fields: &[FieldValue]) -> Option<String> {
    text_field(fields, 0)
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

fn strip_ansi(text: &str) -> String {
    String::from_utf8(strip_ansi_escapes::strip(text)).unwrap_or_else(|_| text.to_owned())
}

fn parse_builder_failure(text: &str) -> Option<BuilderFailure> {
    let after_prefix = text.strip_prefix("error: builder for '")?;
    let (derivation, _) = after_prefix.split_once("' failed with exit code ")?;
    if derivation.ends_with(".drv") {
        Some(BuilderFailure {
            derivation: derivation.to_owned(),
        })
    } else {
        None
    }
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
}
