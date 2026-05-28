#![allow(clippy::module_name_repetitions)]

use std::collections::{BTreeMap, BTreeSet};
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

/// Same idea for `errors`; these grow once per error-level `msg` event,
/// which is bounded but unfriendly for warning-heavy evals or long fetches.
const STATE_ERROR_RETAIN: usize = 2_000;

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

/// One incremental change to the monitor state, mirroring the snapshot shape.
///
/// The state machine accumulates these as it applies each Nix log line, so the
/// transport can ship only what changed instead of re-broadcasting the whole
/// snapshot per line (the O(n²) cost called out on [`MonitorState::logs`]). A
/// freshly-connected client receives one [`Delta::Reset`] seed, then the live
/// stream of the variants below. The discriminant rides in a `type` field so
/// the browser can decode it as a tagged union.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Delta {
    /// Full state, used only to seed a new subscriber. Never broadcast.
    Reset { snapshot: MonitorSnapshot },
    /// A build was created or changed (status, phase, host, failure).
    BuildUpsert { build: BuildNode },
    /// An activity was created or changed (status, phase, progress).
    ActivityUpsert { activity: ActivityNode },
    /// New log entries appended since the last delta. The client mirrors the
    /// per-build `logCount` from these so the hot path carries no `BuildUpsert`.
    LogsAppend { entries: Vec<LogEntry> },
    /// The aggregate progress line moved.
    ProgressSet { progress: ActivityProgress },
    /// The expected count for one activity type was (re)declared.
    ExpectedSet { name: String, value: i64 },
    /// One operator/error message was appended to the errors list.
    ErrorAppend { message: String },
    /// The dependency DAG was recomputed after learning new edges.
    DependenciesSet { edges: Vec<DerivationEdge> },
    /// The wrapped Nix process exited; `exit_code` is its status if any.
    Finished { exit_code: Option<i32> },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorState {
    pub activities: BTreeMap<u64, ActivityNode>,
    pub builds: BTreeMap<String, BuildNode>,
    pub logs: Vec<LogEntry>,
    pub errors: Vec<String>,
    pub progress: Option<ActivityProgress>,
    pub expected: BTreeMap<String, i64>,
    pub exit_code: Option<i32>,
    pub finished: bool,
    /// Direct input `.drv` paths per derivation, learned out-of-band from
    /// `nix-store --query --references` (the internal-json stream carries no
    /// dependency edges). The snapshot turns this into the rendered DAG; the
    /// raw adjacency stays server-side and never rides the wire.
    direct_deps: BTreeMap<String, Vec<String>>,
    /// Monotonic counter for `LogEntry.index`. Kept independent of
    /// `logs.len()` so retention pruning never reuses an index.
    log_counter: u64,
    /// Deltas accumulated since the last [`MonitorState::drain_deltas`]. Each
    /// mutating method pushes the changes it makes here; the transport drains
    /// and broadcasts them. Never serialized: it is transient outbound state,
    /// not part of the snapshot.
    #[serde(skip)]
    outbox: Vec<Delta>,
}

impl MonitorState {
    #[must_use]
    pub fn snapshot(&self) -> MonitorSnapshot {
        let log_tail_start = self.logs.len().saturating_sub(SNAPSHOT_LOG_LIMIT);
        let observed: BTreeSet<&str> = self.builds.keys().map(String::as_str).collect();
        MonitorSnapshot {
            activities: self.activities.values().cloned().collect(),
            builds: self.builds.values().cloned().collect(),
            logs: self.logs[log_tail_start..].to_vec(),
            errors: self.errors.clone(),
            progress: self.progress,
            expected: self.expected.clone(),
            dependencies: dependency_edges(&self.direct_deps, &observed),
            exit_code: self.exit_code,
            finished: self.finished,
        }
    }

    /// Take the deltas accumulated since the last drain. The transport calls
    /// this after every applied line and broadcasts the result; a [`Reset`] seed
    /// is built separately from [`snapshot`], so it never appears here.
    ///
    /// [`Reset`]: Delta::Reset
    /// [`snapshot`]: MonitorState::snapshot
    pub fn drain_deltas(&mut self) -> Vec<Delta> {
        std::mem::take(&mut self.outbox)
    }

    fn emit(&mut self, delta: Delta) {
        self.outbox.push(delta);
    }

    /// Emit the current build node for `derivation` if it exists. Used after a
    /// mutation that changed a build so the wire carries the new row.
    fn emit_build(&mut self, derivation: &str) {
        if let Some(build) = self.builds.get(derivation).cloned() {
            self.emit(Delta::BuildUpsert { build });
        }
    }

    /// Emit the current activity node for `id` if it exists.
    fn emit_activity(&mut self, id: u64) {
        if let Some(activity) = self.activities.get(&id).cloned() {
            self.emit(Delta::ActivityUpsert { activity });
        }
    }

    /// Record the direct input derivations of one built derivation, learned
    /// from `nix-store --query --references`. Only the `.drv` inputs matter for
    /// the dependency DAG; the caller filters source paths out before calling.
    /// Stored unfiltered by observation status so a dependency that starts
    /// building later still produces its edge on the next snapshot.
    pub fn record_direct_dependencies(&mut self, derivation: String, input_drvs: Vec<String>) {
        self.direct_deps.insert(derivation, input_drvs);
        let observed: BTreeSet<&str> = self.builds.keys().map(String::as_str).collect();
        let edges = dependency_edges(&self.direct_deps, &observed);
        self.emit(Delta::DependenciesSet { edges });
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
                self.push_error(format!("failed to parse Nix event: {error}"));
                self.push_log(None, None, text);
            }
        }
    }

    /// Append one operator/error message and emit it. Caps the retained list
    /// from the head; the client mirrors the same cap, so no trim delta ships.
    fn push_error(&mut self, message: String) {
        self.errors.push(message.clone());
        truncate_head(&mut self.errors, STATE_ERROR_RETAIN);
        self.emit(Delta::ErrorAppend { message });
    }

    /// Settle the run and, on a clean exit, promote `Stopped` builds to
    /// `Succeeded`. Nix has no positive success marker per activity, so we
    /// wait for the process to confirm before claiming success.
    pub fn finish(&mut self, exit_code: Option<i32>) {
        self.exit_code = exit_code;
        self.finished = true;
        if exit_code == Some(0) {
            let promoted: Vec<String> = self
                .builds
                .iter_mut()
                .filter(|(_, build)| build.status == BuildStatus::Stopped)
                .map(|(derivation, build)| {
                    build.status = BuildStatus::Succeeded;
                    derivation.clone()
                })
                .collect();
            for derivation in promoted {
                self.emit_build(&derivation);
            }
        }
        self.emit(Delta::Finished { exit_code });
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
                started_at_ms: now_ms,
                stopped_at_ms: None,
                build: build.clone(),
            },
        );

        if let Some(derivation) = build {
            self.builds.insert(
                derivation.clone(),
                BuildNode {
                    derivation: derivation.clone(),
                    activity_id: Some(action.id),
                    host,
                    phase: None,
                    status: BuildStatus::Running,
                    log_count: 0,
                    started_at_ms: now_ms,
                    stopped_at_ms: None,
                },
            );
            self.emit_build(&derivation);
        }
        self.emit_activity(action.id);
    }

    /// Mark the activity stopped. The build status moves to `Stopped` and
    /// stays there until either a builder failure arrives (`Failed`) or the
    /// process exits cleanly (`finish` promotes to `Succeeded`). Nix never
    /// emits a per-activity success signal, so we cannot do better without
    /// inventing one.
    fn stop_activity(&mut self, id: u64) {
        let now_ms = current_unix_ms();
        let Some(activity) = self.activities.get_mut(&id) else {
            return;
        };
        activity.status = ActivityStatus::Stopped;
        activity.stopped_at_ms = Some(now_ms);
        let stopped_build = if let Some(build) = activity.build.clone()
            && let Some(build_node) = self.builds.get_mut(&build)
            && build_node.status == BuildStatus::Running
        {
            build_node.status = BuildStatus::Stopped;
            build_node.stopped_at_ms = Some(now_ms);
            Some(build)
        } else {
            None
        };
        self.emit_activity(id);
        if let Some(build) = stopped_build {
            self.emit_build(&build);
        }
    }

    fn apply_result(&mut self, action: &ResultAction) {
        match &action.result {
            ActivityResult::BuildLogLine { line } | ActivityResult::PostBuildLogLine { line } => {
                let cleaned = strip_ansi(line);
                self.push_log(Some(action.id), None, &cleaned);
            }
            ActivityResult::SetPhase { phase } => {
                let Some(activity) = self.activities.get_mut(&action.id) else {
                    return;
                };
                activity.phase = Some(phase.clone());
                let changed_build = activity.build.clone();
                if let Some(build) = &changed_build
                    && let Some(build_node) = self.builds.get_mut(build)
                {
                    build_node.phase = Some(phase.clone());
                }
                self.emit_activity(action.id);
                if let Some(build) = changed_build {
                    self.emit_build(&build);
                }
            }
            ActivityResult::Progress { progress } => {
                self.progress = Some(*progress);
                if let Some(activity) = self.activities.get_mut(&action.id) {
                    activity.progress = Some(*progress);
                }
                self.emit(Delta::ProgressSet {
                    progress: *progress,
                });
                self.emit_activity(action.id);
            }
            ActivityResult::SetExpected {
                activity_type,
                expected,
            } => {
                self.expected.insert(activity_type.name.clone(), *expected);
                self.emit(Delta::ExpectedSet {
                    name: activity_type.name.clone(),
                    value: *expected,
                });
            }
            ActivityResult::FetchStatus { status } => {
                // Surface substituter fetch status in the log stream. It used
                // to land in a `messages` bin the UI never rendered, so the
                // operator never saw "downloading ..." progress.
                let cleaned = strip_ansi(status);
                self.push_log(Some(action.id), None, &cleaned);
            }
            ActivityResult::FileLinked { .. } | ActivityResult::Other { .. } => {}
        }
    }

    fn apply_message(&mut self, action: &MessageAction) {
        let cleaned = Self::cleaned_message(action);
        if action.level == Some(NIX_LEVEL_ERROR) {
            self.push_error(cleaned.clone());
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
            self.mark_failed_build(&failure);
        }
    }

    fn apply_plain(&mut self, text: &str) {
        let stripped = strip_ansi(text);
        if let Some(failure) = parse_builder_failure(&stripped) {
            self.mark_failed_build(&failure);
        }
        self.push_log(None, None, &stripped);
    }

    fn mark_failed_build(&mut self, failure: &BuilderFailure) {
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
                    derivation: failure.derivation.clone(),
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
        self.emit_build(&failure.derivation);
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

        let entry = LogEntry {
            index,
            activity_id,
            level,
            text: text.to_owned(),
        };
        self.logs.push(entry.clone());
        truncate_head(&mut self.logs, STATE_LOG_RETAIN);
        self.emit(Delta::LogsAppend {
            entries: vec![entry],
        });
    }
}

/// Drop entries from the front so the collection never exceeds `max`.
/// Cheap when `max - new.len()` is small (the common steady-state).
fn truncate_head<T>(items: &mut Vec<T>, max: usize) {
    if items.len() > max {
        items.drain(..items.len() - max);
    }
}

/// Build the minimal dependency DAG over the derivations Nix actually built.
///
/// `direct_deps` maps a derivation to the input `.drv` paths it directly
/// requires. An edge survives only when both endpoints are `observed` (a real
/// build, so it has a row to attach to); a link bridged entirely through a
/// cached, never-built derivation is intentionally absent. The result is
/// transitively reduced, so a chain `a -> b -> c` never also keeps the
/// redundant `a -> c`, and sorted for a stable wire order.
fn dependency_edges(
    direct_deps: &BTreeMap<String, Vec<String>>,
    observed: &BTreeSet<&str>,
) -> Vec<DerivationEdge> {
    let mut adjacency: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (from, inputs) in direct_deps {
        if !observed.contains(from.as_str()) {
            continue;
        }
        for to in inputs {
            if from != to && observed.contains(to.as_str()) {
                adjacency.entry(from).or_default().insert(to);
            }
        }
    }

    let mut edges = Vec::new();
    for (&from, targets) in &adjacency {
        for &to in targets {
            if !reachable_via_detour(&adjacency, from, to) {
                edges.push(DerivationEdge {
                    from: from.to_owned(),
                    to: to.to_owned(),
                });
            }
        }
    }
    edges.sort();
    edges
}

/// Whether `to` is reachable from `from` through a path of length two or more,
/// i.e. via some neighbour other than `to` itself. Such a `from -> to` edge is
/// implied by the longer path and is dropped in transitive reduction.
fn reachable_via_detour(adjacency: &BTreeMap<&str, BTreeSet<&str>>, from: &str, to: &str) -> bool {
    let Some(neighbours) = adjacency.get(from) else {
        return false;
    };
    neighbours
        .iter()
        .any(|&next| next != to && reaches(adjacency, next, to))
}

/// Depth-first reachability from `start` to `target`. The `visited` guard keeps
/// the walk finite even though derivation graphs are acyclic by construction.
fn reaches(adjacency: &BTreeMap<&str, BTreeSet<&str>>, start: &str, target: &str) -> bool {
    let mut stack = vec![start];
    let mut visited: BTreeSet<&str> = BTreeSet::new();
    while let Some(node) = stack.pop() {
        if node == target {
            return true;
        }
        if !visited.insert(node) {
            continue;
        }
        if let Some(neighbours) = adjacency.get(node) {
            stack.extend(neighbours.iter().copied());
        }
    }
    false
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorSnapshot {
    pub activities: Vec<ActivityNode>,
    pub builds: Vec<BuildNode>,
    pub logs: Vec<LogEntry>,
    pub errors: Vec<String>,
    pub progress: Option<ActivityProgress>,
    pub expected: BTreeMap<String, i64>,
    /// Minimal dependency DAG over derivations Nix actually built: each edge's
    /// `from` directly requires `to`. Derived from `direct_deps` at snapshot
    /// time, restricted to built derivations and transitively reduced.
    pub dependencies: Vec<DerivationEdge>,
    pub exit_code: Option<i32>,
    pub finished: bool,
}

/// One directed dependency edge: `from` directly requires `to`. Both endpoints
/// are `derivation` paths matching a [`BuildNode`], so the UI can join edges to
/// build rows by string identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DerivationEdge {
    pub from: String,
    pub to: String,
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
            let activity_type_code =
                u64::try_from(activity_type_code).map_err(|_| ParseError::NegativeActivityType)?;
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
            .map_or_else(|| FieldValue::Other(value.clone()), FieldValue::Number),
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

/// Strip CSI sequences from `text`.
///
/// Handles both ESC-prefixed CSI sequences and the bare `[<n>;<n>m` form that
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

// Nix always emits `/nix/store/.../*.drv` lowercase, so a plain byte suffix
// match is the right comparison; the case-insensitive clippy lint is a false
// positive for this format.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
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
        state.apply_line(r#"@nix {"action":"msg","level":0,"msg":"something went wrong"}"#);
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
    fn error_retention_caps_errors_array() {
        let mut state = MonitorState::default();
        for i in 0..(STATE_ERROR_RETAIN + 25) {
            state.apply_line(&format!(
                "@nix {{\"action\":\"msg\",\"level\":0,\"msg\":\"boom {i}\"}}"
            ));
        }
        assert_eq!(state.snapshot().errors.len(), STATE_ERROR_RETAIN);
    }

    fn edge(from: &str, to: &str) -> DerivationEdge {
        DerivationEdge {
            from: from.to_owned(),
            to: to.to_owned(),
        }
    }

    fn deps(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(drv, inputs)| {
                (
                    (*drv).to_owned(),
                    inputs.iter().map(|s| (*s).to_owned()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn dependency_edges_drop_redundant_transitive_links() {
        // a -> b -> c with a direct a -> c shortcut. Reduction keeps the chain
        // and drops the shortcut.
        let direct = deps(&[("a", &["b", "c"]), ("b", &["c"])]);
        let observed = BTreeSet::from(["a", "b", "c"]);
        assert_eq!(
            dependency_edges(&direct, &observed),
            vec![edge("a", "b"), edge("b", "c")]
        );
    }

    #[test]
    fn dependency_edges_keep_both_arms_of_a_diamond() {
        // a depends on b and c, both of which depend on d. Neither arm is
        // implied by the other, so all four edges survive.
        let direct = deps(&[("a", &["b", "c"]), ("b", &["d"]), ("c", &["d"])]);
        let observed = BTreeSet::from(["a", "b", "c", "d"]);
        assert_eq!(
            dependency_edges(&direct, &observed),
            vec![
                edge("a", "b"),
                edge("a", "c"),
                edge("b", "d"),
                edge("c", "d")
            ]
        );
    }

    #[test]
    fn dependency_edges_ignore_unbuilt_endpoints() {
        // `x` was never built (not observed), so the a -> x edge vanishes and a
        // is left depending only on the observed b.
        let direct = deps(&[("a", &["b", "x"])]);
        let observed = BTreeSet::from(["a", "b"]);
        assert_eq!(dependency_edges(&direct, &observed), vec![edge("a", "b")]);
    }

    #[test]
    fn snapshot_exposes_recorded_dependency_dag() {
        let mut state = MonitorState::default();
        for (id, drv) in [
            (1u64, "/nix/store/aaa-app.drv"),
            (2, "/nix/store/bbb-lib.drv"),
        ] {
            state.apply_line(&format!(
                r#"@nix {{"action":"start","fields":["{drv}","local",1,1],"id":{id},"level":3,"text":"building","type":105}}"#
            ));
        }
        state.record_direct_dependencies(
            "/nix/store/aaa-app.drv".to_owned(),
            vec!["/nix/store/bbb-lib.drv".to_owned()],
        );

        assert_eq!(
            state.snapshot().dependencies,
            vec![edge("/nix/store/aaa-app.drv", "/nix/store/bbb-lib.drv")]
        );
    }

    #[test]
    fn build_start_emits_build_and_activity_upserts() {
        let mut state = MonitorState::default();
        state.apply_line(r#"@nix {"action":"start","fields":["/nix/store/abc-demo.drv","ssh://builder",1,1],"id":7,"level":3,"text":"building","type":105}"#);

        let deltas = state.drain_deltas();
        let build = deltas.iter().find_map(|delta| match delta {
            Delta::BuildUpsert { build } => Some(build),
            _ => None,
        });
        assert_eq!(
            build.map(|build| (build.derivation.as_str(), build.host.as_deref())),
            Some(("/nix/store/abc-demo.drv", Some("ssh://builder"))),
            "build start carries the derivation and host on a BuildUpsert"
        );
        assert!(
            deltas
                .iter()
                .any(|delta| matches!(delta, Delta::ActivityUpsert { activity } if activity.id == 7)),
            "build start also upserts its activity row"
        );
    }

    #[test]
    fn log_line_emits_one_logs_append() {
        let mut state = MonitorState::default();
        state.apply_line(r#"@nix {"action":"start","fields":["/nix/store/abc-demo.drv","local",1,1],"id":7,"level":3,"text":"building","type":105}"#);
        state.drain_deltas();
        state.apply_line(r#"@nix {"action":"result","fields":["compiling"],"id":7,"type":101}"#);

        let appends: Vec<Vec<LogEntry>> = state
            .drain_deltas()
            .into_iter()
            .filter_map(|delta| match delta {
                Delta::LogsAppend { entries } => Some(entries),
                _ => None,
            })
            .collect();
        assert_eq!(appends.len(), 1, "one log line yields exactly one LogsAppend");
        assert_eq!(appends[0].len(), 1);
        assert_eq!(appends[0][0].text, "compiling");
    }

    #[test]
    fn finish_promotes_stopped_build_then_signals_finished() {
        let mut state = MonitorState::default();
        state.apply_line(r#"@nix {"action":"start","fields":["/nix/store/abc-demo.drv","local",1,1],"id":7,"level":3,"text":"building","type":105}"#);
        state.apply_line(r#"@nix {"action":"stop","id":7}"#);
        state.drain_deltas();

        state.finish(Some(0));
        let deltas = state.drain_deltas();

        let promoted = deltas.iter().find_map(|delta| match delta {
            Delta::BuildUpsert { build } => Some(build.status),
            _ => None,
        });
        assert_eq!(
            promoted,
            Some(BuildStatus::Succeeded),
            "a clean finish promotes the stopped build via BuildUpsert"
        );
        assert!(
            matches!(deltas.last(), Some(Delta::Finished { exit_code: Some(0) })),
            "Finished is the terminal delta"
        );
    }

    #[test]
    fn drain_clears_the_outbox() {
        let mut state = MonitorState::default();
        state.apply_line(r#"@nix {"action":"start","fields":["/nix/store/abc-demo.drv","local",1,1],"id":7,"level":3,"text":"building","type":105}"#);
        assert!(!state.drain_deltas().is_empty(), "first drain yields the start deltas");
        assert!(
            state.drain_deltas().is_empty(),
            "a second drain with no intervening mutation is empty"
        );
    }
}
