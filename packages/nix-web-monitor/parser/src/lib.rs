#![allow(clippy::module_name_repetitions)]

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use snafu::Snafu;

pub mod daemon;
pub use daemon::{DaemonInfo, DaemonOps, OpClass};

const NIX_JSON_PREFIX: &str = "@nix ";

/// Suffix Nix uses for derivation files. Build-plan lines and closure queries
/// both carry a mix of `.drv` inputs and source paths; only the `.drv` paths
/// are nodes in the build DAG.
const DRV_SUFFIX: &str = ".drv";

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
    /// Nix emits this right before building a content-addressed derivation: it
    /// "resolves" the derivation (rewrites each input reference to the input's
    /// actual output) into a second `.drv` with a different hash, then builds
    /// that resolved drv. `fields[0]` is the original (requested) drv, `fields[1]`
    /// the resolved one. We fold the two so a CA build shows one row, not a
    /// look-alike pair. See [`MonitorState::resolved_to_original`].
    pub const RESOLVE: u64 = 111;
}

/// Where the message stream sits relative to a build-plan announcement. Nix
/// prints "these N derivations will be built:" (or the singular form) followed
/// by one indented `.drv` per line, and a separate "will be fetched" block for
/// substituted paths. Only the indented lines under [`PlanSection::Build`] name
/// derivations that will actually be built.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PlanSection {
    /// Not currently reading a plan block.
    #[default]
    None,
    /// Inside the "will be built" derivation list.
    Build,
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

    #[snafu(display("expected a byte count and an optional block count"))]
    FileLinkedFields,

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
    /// One duplicate store file Nix replaced with a hard link.
    ///
    /// Nix emits this per linked file during `actOptimiseStore`, with the
    /// file's apparent size and (off Windows) its block count -- the space that
    /// hard-linking reclaims. The state machine folds these into
    /// [`OptimiseStats`] so the run-wide hard-linking cost is visible;
    /// auto-optimise-store does this work inline on every store add, which is a
    /// common reason a "copying ... to the store" step is slow.
    FileLinked {
        bytes: i64,
        blocks: Option<i64>,
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

/// Run-wide store-optimisation totals.
///
/// Accumulated from the per-file [`ActivityResult::FileLinked`] events Nix
/// emits while hard-linking duplicate store files. Nix reports no aggregate, so
/// the state machine sums one here: `files_linked` counts the events and
/// `bytes_freed` sums their apparent sizes (the space hard-linking reclaims).
/// Carried in the snapshot like [`ActivityProgress`] so the UI can show how
/// much store optimisation a run did -- the otherwise-invisible cost behind a
/// slow "copying to the store".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptimiseStats {
    pub files_linked: u64,
    pub bytes_freed: i64,
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
    /// Run-wide store-optimisation totals moved (a file was hard-linked).
    OptimiseSet { optimise: OptimiseStats },
    /// The live nix-daemon syscall view changed (new counts, path, or status).
    DaemonSet { daemon: DaemonInfo },
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
    /// Run-wide store-optimisation totals, summed from `FileLinked` events.
    pub optimise: OptimiseStats,
    /// Live nix-daemon syscall view, fed out-of-band by the server's tracer.
    pub daemon: DaemonInfo,
    pub expected: BTreeMap<String, i64>,
    pub exit_code: Option<i32>,
    pub finished: bool,
    /// The Nix invocation being monitored (e.g. `nix build .#ix`), shown as the
    /// label of the single tree root so the whole build hangs under the goal.
    /// Constant for a run; set once via [`MonitorState::new`].
    command: String,
    /// Transitive input `.drv` closure per derivation, learned out-of-band from
    /// `nix-store --query --requisites` (the internal-json stream carries no
    /// dependency edges). Because the closure is transitive, the snapshot can
    /// reduce it to a DAG whose edges connect built derivations even when the
    /// path between them runs through cached intermediates that Nix never
    /// reports. The raw closure stays server-side and never rides the wire.
    closure_deps: BTreeMap<String, BTreeSet<String>>,
    /// Last dependency edge set broadcast, so a query that does not change the
    /// rendered DAG (common once the closure is mostly known) skips re-emitting
    /// an identical `DependenciesSet`.
    #[serde(skip)]
    last_dependencies: Option<Vec<DerivationEdge>>,
    /// Which section of a build-plan announcement the message stream is inside.
    /// Nix prints the plan as a header line followed by indented store paths;
    /// this tracks that the indented lines belong to the build list rather than
    /// the "will be fetched" list or unrelated output.
    #[serde(skip)]
    plan_section: PlanSection,
    /// Maps a resolved derivation back to the original (content-addressed) one
    /// Nix resolved it from. A CA build emits `resolved derivation: A -> B` and
    /// then builds `B`; folding `B`'s build onto `A` keeps the row the user
    /// asked for (`.#dashboard` is `A`) instead of a second look-alike for `B`.
    /// Nix logs the resolve immediately before the build, so the alias is always
    /// known by the time `B`'s build activity starts.
    #[serde(skip)]
    resolved_to_original: BTreeMap<String, String>,
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
    /// Construct a monitor for one Nix invocation. `command` is the displayed
    /// build label (e.g. `nix build .#ix`); everything else starts empty.
    #[must_use]
    pub fn new(command: String) -> Self {
        Self {
            command,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> MonitorSnapshot {
        let log_tail_start = self.logs.len().saturating_sub(SNAPSHOT_LOG_LIMIT);
        let observed: BTreeSet<&str> = self.builds.keys().map(String::as_str).collect();
        MonitorSnapshot {
            command: self.command.clone(),
            activities: self.activities.values().cloned().collect(),
            builds: self.builds.values().cloned().collect(),
            logs: self.logs[log_tail_start..].to_vec(),
            errors: self.errors.clone(),
            progress: self.progress,
            optimise: self.optimise,
            daemon: self.daemon.clone(),
            expected: self.expected.clone(),
            dependencies: dependency_edges(&self.closure_deps, &observed),
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

    /// Attach a measured byte size to a "copying <path> to the store" activity
    /// and re-broadcast the row. The server measures the source itself because
    /// Nix reports that copy with no byte progress (see [`copy_to_store_source`]);
    /// this is the only place `size_bytes` is set. Activities are never removed,
    /// so a slow measurement that lands after the copy stopped still annotates
    /// the (stopped) row; the only no-op is an id that was never seen.
    pub fn set_activity_size(&mut self, id: u64, size_bytes: i64) {
        let Some(activity) = self.activities.get_mut(&id) else {
            return;
        };
        activity.size_bytes = Some(size_bytes);
        self.emit_activity(id);
    }

    /// Replace the live nix-daemon syscall view and broadcast the change.
    ///
    /// Called by the server's tracer on its sampling timer. Skips the broadcast
    /// when nothing changed so an idle daemon (or a tracer that cannot attach)
    /// does not put a frame on the wire every tick; the snapshot still carries
    /// the latest value for a freshly-connected client.
    pub fn set_daemon(&mut self, daemon: DaemonInfo) {
        if self.daemon == daemon {
            return;
        }
        self.daemon = daemon;
        self.emit(Delta::DaemonSet {
            daemon: self.daemon.clone(),
        });
    }

    /// Record the transitive input `.drv` closure of one derivation, learned
    /// from `nix-store --query --requisites`. The caller filters source paths
    /// and the derivation itself out before calling. Stored unfiltered by build
    /// status so the DAG can bridge through cached intermediates: an edge
    /// between two built derivations survives even when the path between them
    /// runs entirely through derivations Nix never reports building.
    pub fn record_closure(&mut self, derivation: String, closure_drvs: BTreeSet<String>) {
        self.closure_deps.insert(derivation, closure_drvs);
        let observed: BTreeSet<&str> = self.builds.keys().map(String::as_str).collect();
        let edges = dependency_edges(&self.closure_deps, &observed);
        self.emit_dependencies(edges);
    }

    /// Broadcast a recomputed edge set only when it differs from the last one.
    /// Closure queries arrive faster than the rendered DAG actually changes, so
    /// this drops the redundant `DependenciesSet` frames a naive recompute would
    /// put on the wire for every cached intermediate that touched no edge.
    fn emit_dependencies(&mut self, edges: Vec<DerivationEdge>) {
        if self.last_dependencies.as_ref() == Some(&edges) {
            return;
        }
        self.last_dependencies = Some(edges.clone());
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

    /// Settle the run and, on a clean exit, promote `Stopped` and still-`Planned`
    /// builds to `Succeeded`. Nix has no positive success marker per activity, so
    /// we wait for the process to confirm before claiming success; a clean exit
    /// also means any node the plan announced was realised, so leftover planned
    /// rows resolve to success rather than lingering as pending work.
    pub fn finish(&mut self, exit_code: Option<i32>) {
        self.exit_code = exit_code;
        self.finished = true;
        if exit_code == Some(0) {
            let promoted: Vec<String> = self
                .builds
                .iter_mut()
                .filter(|(_, build)| {
                    matches!(build.status, BuildStatus::Stopped | BuildStatus::Planned)
                })
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
        let raw_build = if action.activity_type.code == activity_code::BUILD {
            text_field(&action.fields, 0)
        } else {
            None
        };
        let host = if action.activity_type.code == activity_code::BUILD {
            text_field(&action.fields, 1)
        } else {
            None
        };
        // A content-addressed build runs under its resolved drv; fold it onto the
        // original so the row stays the one the user asked for, flagged `ca`. The
        // resolve message always precedes the build, so the alias is known here.
        let content_addressed = raw_build
            .as_ref()
            .is_some_and(|drv| self.resolved_to_original.contains_key(drv));
        let build =
            raw_build.map(|drv| self.resolved_to_original.get(&drv).cloned().unwrap_or(drv));

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
                // Point the activity at the folded derivation so stop/finish and
                // log attribution resolve to the same row the build uses.
                build: build.clone(),
                size_bytes: None,
            },
        );

        self.note_resolved_derivation(action);

        if let Some(derivation) = build {
            use std::collections::btree_map::Entry;
            match self.builds.entry(derivation.clone()) {
                // A planned node lights up: stamp the real start so its duration
                // counts from now rather than from when the plan announced it,
                // and keep any log count already attributed to it.
                Entry::Occupied(mut entry) => {
                    let node = entry.get_mut();
                    node.activity_id = Some(action.id);
                    node.host = host;
                    node.status = BuildStatus::Running;
                    node.started_at_ms = now_ms;
                    node.stopped_at_ms = None;
                    node.content_addressed |= content_addressed;
                }
                Entry::Vacant(entry) => {
                    entry.insert(BuildNode {
                        derivation: derivation.clone(),
                        activity_id: Some(action.id),
                        host,
                        phase: None,
                        status: BuildStatus::Running,
                        log_count: 0,
                        started_at_ms: now_ms,
                        stopped_at_ms: None,
                        content_addressed,
                    });
                }
            }
            self.emit_build(&derivation);
        }
        self.emit_activity(action.id);
    }

    /// Record a `resolved derivation: A -> B` activity: alias the resolved drv
    /// `B` back to the original `A` and flag `A` content-addressed. The build
    /// activity for `B` arrives next and folds onto `A` via the alias.
    fn note_resolved_derivation(&mut self, action: &StartAction) {
        if action.activity_type.code != activity_code::RESOLVE {
            return;
        }
        let (Some(original), Some(resolved)) =
            (text_field(&action.fields, 0), text_field(&action.fields, 1))
        else {
            return;
        };
        self.resolved_to_original.insert(resolved, original.clone());
        let flagged = match self.builds.get_mut(&original) {
            Some(node) => {
                node.content_addressed = true;
                true
            }
            None => false,
        };
        if flagged {
            self.emit_build(&original);
        }
    }

    /// Seed a planned build node from the "will be built" announcement. No-op if
    /// the derivation already has a row, so a build that starts before (or
    /// without) the plan line keeps its live status.
    fn plan_build(&mut self, derivation: &str) {
        use std::collections::btree_map::Entry;
        let Entry::Vacant(entry) = self.builds.entry(derivation.to_owned()) else {
            return;
        };
        entry.insert(BuildNode {
            derivation: derivation.to_owned(),
            activity_id: None,
            host: None,
            phase: None,
            status: BuildStatus::Planned,
            log_count: 0,
            // Plan time, overwritten with the real start in `start_activity`;
            // the UI shows no duration while a node is still planned.
            started_at_ms: current_unix_ms(),
            stopped_at_ms: None,
            content_addressed: false,
        });
        self.emit_build(derivation);
    }

    /// Fold a parsed `msg` line into the build-plan tracker. Nix announces every
    /// derivation it will build up front, so seeding those as planned nodes
    /// renders the whole tree (target at the root, dependencies nested beneath)
    /// before any leaf starts, instead of growing it bottom-up as builds begin.
    fn note_build_plan(&mut self, text: &str) {
        if is_build_plan_header(text) {
            self.plan_section = PlanSection::Build;
            return;
        }
        if self.plan_section == PlanSection::Build
            && let Some(derivation) = planned_derivation(text)
        {
            self.plan_build(derivation);
            return;
        }
        // Any line that is not an indented plan entry ends the build list: the
        // "will be fetched" header, a blank line, or unrelated output.
        if !text.starts_with(char::is_whitespace) {
            self.plan_section = PlanSection::None;
        }
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
            ActivityResult::FileLinked { bytes, .. } => {
                // Each event is one duplicate store file replaced by a hard link;
                // sum the count and the reclaimed bytes so the run-wide
                // optimisation cost is visible. `bytes` is an apparent file size
                // and never negative, but clamp defensively before summing.
                self.optimise.files_linked = self.optimise.files_linked.saturating_add(1);
                self.optimise.bytes_freed = self.optimise.bytes_freed.saturating_add((*bytes).max(0));
                self.emit(Delta::OptimiseSet {
                    optimise: self.optimise,
                });
            }
            ActivityResult::Other { .. } => {}
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

        // Seed planned build nodes from the up-front "will be built" plan.
        self.note_build_plan(&cleaned);

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
        // A CA failure names the resolved drv; fold it onto the original row.
        let original = self.resolved_to_original.get(&failure.derivation).cloned();
        let content_addressed = original.is_some();
        let derivation = original.unwrap_or_else(|| failure.derivation.clone());
        match self.builds.entry(derivation.clone()) {
            Entry::Occupied(mut entry) => {
                let build = entry.get_mut();
                build.status = BuildStatus::Failed;
                if build.stopped_at_ms.is_none() {
                    build.stopped_at_ms = Some(now_ms);
                }
                build.content_addressed |= content_addressed;
            }
            Entry::Vacant(entry) => {
                entry.insert(BuildNode {
                    derivation: derivation.clone(),
                    activity_id: None,
                    host: None,
                    phase: None,
                    status: BuildStatus::Failed,
                    log_count: 0,
                    started_at_ms: now_ms,
                    stopped_at_ms: Some(now_ms),
                    content_addressed,
                });
            }
        }
        self.emit_build(&derivation);
    }

    fn push_log(&mut self, activity_id: Option<u64>, level: Option<i64>, text: &str) {
        #[expect(
            clippy::fallible_int_fallback,
            reason = "log_counter is a u64 index that always fits usize on the repo's 64-bit targets"
        )]
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

/// Reduce the transitive `.drv` closures into the minimal dependency DAG over
/// the derivations the UI renders (`rendered`: planned and built rows).
///
/// `closure_deps` maps a derivation to its full transitive input `.drv`
/// closure. Restricting each closure to `rendered` yields every rendered
/// derivation reachable from `from`, even when the only path between them runs
/// through cached intermediates Nix never reports building. That is what keeps a
/// deep dependency nested under the target instead of floating up as its own
/// root. The relation is transitively reduced, so a chain `a -> b -> c` never
/// also keeps the redundant `a -> c`, and sorted for a stable wire order.
fn dependency_edges(
    closure_deps: &BTreeMap<String, BTreeSet<String>>,
    rendered: &BTreeSet<&str>,
) -> Vec<DerivationEdge> {
    let mut edges = Vec::new();
    for (from, closure) in closure_deps {
        if !rendered.contains(from.as_str()) {
            continue;
        }
        // Rendered derivations reachable from `from`, the candidate edge targets.
        let reachable: Vec<&str> = closure
            .iter()
            .map(String::as_str)
            .filter(|to| *to != from.as_str() && rendered.contains(to))
            .collect();
        for &to in &reachable {
            // Drop `from -> to` when another rendered node between them also
            // reaches `to`: the longer path already implies this edge.
            let implied = reachable.iter().any(|&mid| {
                mid != to
                    && closure_deps
                        .get(mid)
                        .is_some_and(|mid_closure| mid_closure.contains(to))
            });
            if !implied {
                edges.push(DerivationEdge {
                    from: from.clone(),
                    to: to.to_owned(),
                });
            }
        }
    }
    edges.sort();
    edges
}

/// Whether `text` is the header that precedes Nix's "will be built" derivation
/// list. Nix uses the plural "these N derivations will be built:" and the
/// singular "this derivation will be built:"; matching the shared suffix covers
/// both without pinning the count wording.
fn is_build_plan_header(text: &str) -> bool {
    text.trim_end().ends_with("will be built:")
}

/// Parse one indented build-plan entry into its `.drv` path. Plan lines are a
/// lone store path indented under the header; a non-indented or non-`.drv` line
/// (such as the "will be fetched" output paths) yields `None`.
fn planned_derivation(text: &str) -> Option<&str> {
    if !text.starts_with(char::is_whitespace) {
        return None;
    }
    let path = text.trim();
    (path.starts_with("/nix/store/") && path.ends_with(DRV_SUFFIX)).then_some(path)
}

/// Extract the local source path from a Nix "copying <path> to the store"
/// activity, or `None` when the path is not one the server can measure.
///
/// Nix emits this for the local source-tree copy as an unstructured `unknown`
/// activity carrying no byte progress, so the server measures the path itself to
/// show how large the copy is (see [`MonitorState::set_activity_size`]). Handles
/// both quote styles Nix uses (`'…'` for a `git+file` flake, `"…"` for a `path:`
/// flake) and trims a trailing slash so the returned path stats cleanly.
///
/// Only an absolute filesystem path (one starting with `/`) is returned. Nix
/// also copies individual files out of fetched flake inputs and prints those
/// with its virtual source-accessor notation, e.g. `copying
/// '«github:NixOS/nixpkgs#…»/pkgs/…/foo.patch' to the store`. That `«…»` path
/// has no on-disk location, so measuring it spams one `No such file or
/// directory` per patch; rejecting non-absolute paths keeps the measurement to
/// the real local source tree the operator cares about.
#[must_use]
pub fn copy_to_store_source(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("copying ")?;
    let quote = rest.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let body = &rest[quote.len_utf8()..];
    let end = body.find(quote)?;
    let path = &body[..end];
    if !path.starts_with('/') || body[end + quote.len_utf8()..].trim_start() != "to the store" {
        return None;
    }
    Some(path.strip_suffix('/').unwrap_or(path))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorSnapshot {
    /// The Nix invocation being monitored, used as the build tree's root label.
    pub command: String,
    pub activities: Vec<ActivityNode>,
    pub builds: Vec<BuildNode>,
    pub logs: Vec<LogEntry>,
    pub errors: Vec<String>,
    pub progress: Option<ActivityProgress>,
    /// Run-wide store-optimisation totals, summed from `FileLinked` events.
    pub optimise: OptimiseStats,
    /// Live nix-daemon syscall view, fed out-of-band by the server's tracer.
    pub daemon: DaemonInfo,
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
    /// Total bytes the server measured for a "copying <path> to the store"
    /// activity. Nix reports that local source copy as an unstructured `unknown`
    /// activity with no byte progress (see [`copy_to_store_source`]), so without
    /// this the operator cannot tell whether the copy moves a megabyte or a
    /// hundred gigabytes. `None` on every other activity and until the
    /// measurement lands.
    pub size_bytes: Option<i64>,
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
    /// True once Nix resolved this derivation before building it, which only
    /// content-addressed derivations do. The row carries a `ca` badge so the
    /// folded resolved build is explained rather than looking like a stray.
    pub content_addressed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildStatus {
    /// Announced in Nix's "these derivations will be built" plan but not yet
    /// started. Seeds the full tree up front so the target and everything under
    /// it is visible before its leaves begin, instead of the tree filling in
    /// bottom-up as each build starts.
    Planned,
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
            let LinkedFile { bytes, blocks } = file_linked_fields(&fields)?;
            ActivityResult::FileLinked { bytes, blocks }
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
            let NumberPair {
                first: activity_type_code,
                second: expected,
            } = two_numbers(&fields)?;
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

/// The two numeric fields extracted from an activity record, in field order.
struct NumberPair {
    first: i64,
    second: i64,
}

fn two_numbers(fields: &[FieldValue]) -> Result<NumberPair, ParseError> {
    match fields {
        [FieldValue::Number(first), FieldValue::Number(second)] => Ok(NumberPair {
            first: *first,
            second: *second,
        }),
        _ => Err(ParseError::TwoNumericFields),
    }
}

/// The fields of a `resFileLinked` result.
struct LinkedFile {
    /// The linked file's apparent size in bytes (the space hard-linking reclaims).
    bytes: i64,
    /// The file's block count, present only on platforms that report one.
    blocks: Option<i64>,
}

/// Parse a `resFileLinked` result's numeric fields.
///
/// Nix omits the block field on Windows (`st_blocks` has no analogue), so it is
/// optional rather than required.
fn file_linked_fields(fields: &[FieldValue]) -> Result<LinkedFile, ParseError> {
    match fields {
        [FieldValue::Number(bytes)] => Ok(LinkedFile {
            bytes: *bytes,
            blocks: None,
        }),
        [FieldValue::Number(bytes), FieldValue::Number(blocks)] => Ok(LinkedFile {
            bytes: *bytes,
            blocks: Some(*blocks),
        }),
        _ => Err(ParseError::FileLinkedFields),
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

#[expect(
    clippy::fallible_int_fallback,
    reason = "a usize always fits in u64 on every supported target, so the fallback is unreachable"
)]
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
    fn folds_resolved_ca_derivation_into_one_row() {
        let original = "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-dashboard-0.1.0.drv";
        let resolved = "/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-dashboard-0.1.0.drv";
        let mut state = MonitorState::default();
        // Nix resolves the CA derivation, then builds the resolved drv.
        state.apply_line(&format!(
            r#"@nix {{"action":"start","fields":["{original}","{resolved}"],"id":1,"level":3,"text":"resolved derivation","type":111}}"#
        ));
        state.apply_line(&format!(
            r#"@nix {{"action":"start","fields":["{resolved}","local",1,1],"id":2,"level":3,"text":"building","type":105}}"#
        ));

        let snapshot = state.snapshot();
        assert_eq!(
            snapshot.builds.len(),
            1,
            "the resolved build folds onto the original instead of adding a look-alike row"
        );
        let build = &snapshot.builds[0];
        assert_eq!(
            build.derivation, original,
            "the surviving row keeps the requested (original) derivation"
        );
        assert!(
            build.content_addressed,
            "a derivation Nix resolved before building is content-addressed"
        );
        assert_eq!(build.status, BuildStatus::Running);
    }

    #[test]
    fn stopping_the_resolved_build_settles_the_folded_row() {
        let original = "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-dashboard-0.1.0.drv";
        let resolved = "/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-dashboard-0.1.0.drv";
        let mut state = MonitorState::default();
        state.apply_line(&format!(
            r#"@nix {{"action":"start","fields":["{original}","{resolved}"],"id":1,"level":3,"text":"resolved derivation","type":111}}"#
        ));
        state.apply_line(&format!(
            r#"@nix {{"action":"start","fields":["{resolved}","local",1,1],"id":2,"level":3,"text":"building","type":105}}"#
        ));
        // The stop carries the resolved build's activity id; it must still reach
        // the folded original row.
        state.apply_line(r#"@nix {"action":"stop","id":2}"#);
        assert_eq!(state.snapshot().builds[0].status, BuildStatus::Stopped);
        state.finish(Some(0));
        assert_eq!(state.snapshot().builds[0].status, BuildStatus::Succeeded);
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

    /// Build a `closure_deps` map from transitive `.drv` closures, the shape the
    /// resolver records from `nix-store --query --requisites`.
    fn closures(pairs: &[(&str, &[&str])]) -> BTreeMap<String, BTreeSet<String>> {
        pairs
            .iter()
            .map(|(drv, reqs)| {
                (
                    (*drv).to_owned(),
                    reqs.iter().map(|s| (*s).to_owned()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn dependency_edges_drop_redundant_transitive_links() {
        // a -> b -> c with a direct a -> c shortcut. Reduction keeps the chain
        // and drops the shortcut.
        let closure = closures(&[("a", &["b", "c"]), ("b", &["c"])]);
        let rendered = BTreeSet::from(["a", "b", "c"]);
        assert_eq!(
            dependency_edges(&closure, &rendered),
            vec![edge("a", "b"), edge("b", "c")]
        );
    }

    #[test]
    fn dependency_edges_keep_both_arms_of_a_diamond() {
        // a depends on b and c, both of which depend on d. Neither arm is
        // implied by the other, so all four edges survive.
        let closure = closures(&[("a", &["b", "c", "d"]), ("b", &["d"]), ("c", &["d"])]);
        let rendered = BTreeSet::from(["a", "b", "c", "d"]);
        assert_eq!(
            dependency_edges(&closure, &rendered),
            vec![
                edge("a", "b"),
                edge("a", "c"),
                edge("b", "d"),
                edge("c", "d")
            ]
        );
    }

    #[test]
    fn dependency_edges_ignore_unrendered_endpoints() {
        // `x` is in the closure but is not a rendered row, so the a -> x edge
        // vanishes and a is left depending only on the rendered b.
        let closure = closures(&[("a", &["b", "x"])]);
        let rendered = BTreeSet::from(["a", "b"]);
        assert_eq!(dependency_edges(&closure, &rendered), vec![edge("a", "b")]);
    }

    #[test]
    fn dependency_edges_bridge_through_cached_intermediate() {
        // a reaches b only through a cached intermediate `m` that Nix never
        // builds, so `m` is neither queried nor rendered. The transitive closure
        // still lists b, so b nests under a instead of floating up as a root.
        let closure = closures(&[("a", &["m", "b"]), ("b", &[])]);
        let rendered = BTreeSet::from(["a", "b"]);
        assert_eq!(dependency_edges(&closure, &rendered), vec![edge("a", "b")]);
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
        state.record_closure(
            "/nix/store/aaa-app.drv".to_owned(),
            BTreeSet::from(["/nix/store/bbb-lib.drv".to_owned()]),
        );

        assert_eq!(
            state.snapshot().dependencies,
            vec![edge("/nix/store/aaa-app.drv", "/nix/store/bbb-lib.drv")]
        );
    }

    #[test]
    fn build_plan_seeds_planned_nodes_before_any_start() {
        let mut state = MonitorState::default();
        state.apply_line(
            r#"@nix {"action":"msg","level":3,"msg":"these 2 derivations will be built:"}"#,
        );
        state.apply_line(r#"@nix {"action":"msg","level":3,"msg":"  /nix/store/aaa-app.drv"}"#);
        state.apply_line(r#"@nix {"action":"msg","level":3,"msg":"  /nix/store/bbb-lib.drv"}"#);

        let snapshot = state.snapshot();
        assert_eq!(snapshot.builds.len(), 2);
        assert!(
            snapshot
                .builds
                .iter()
                .all(|build| build.status == BuildStatus::Planned),
            "plan entries seed nodes the build has not started yet"
        );
    }

    #[test]
    fn fetched_paths_are_not_planned_build_nodes() {
        let mut state = MonitorState::default();
        state.apply_line(
            r#"@nix {"action":"msg","level":3,"msg":"this derivation will be built:"}"#,
        );
        state.apply_line(r#"@nix {"action":"msg","level":3,"msg":"  /nix/store/aaa-app.drv"}"#);
        state.apply_line(r#"@nix {"action":"msg","level":3,"msg":"these 2 paths will be fetched (1.50 MiB download, 5.00 MiB unpacked):"}"#);
        state.apply_line(
            r#"@nix {"action":"msg","level":3,"msg":"  /nix/store/ccc-cached-output"}"#,
        );

        let snapshot = state.snapshot();
        assert_eq!(
            snapshot.builds.len(),
            1,
            "only the will-be-built derivation is a node; fetched output paths are not"
        );
        assert_eq!(snapshot.builds[0].derivation, "/nix/store/aaa-app.drv");
    }

    #[test]
    fn planned_node_lights_up_on_build_start() {
        let mut state = MonitorState::default();
        state.apply_line(
            r#"@nix {"action":"msg","level":3,"msg":"this derivation will be built:"}"#,
        );
        state.apply_line(r#"@nix {"action":"msg","level":3,"msg":"  /nix/store/aaa-app.drv"}"#);
        assert_eq!(state.snapshot().builds[0].status, BuildStatus::Planned);

        state.apply_line(r#"@nix {"action":"start","fields":["/nix/store/aaa-app.drv","ssh://builder",1,1],"id":7,"level":3,"text":"building","type":105}"#);
        let build = state.snapshot().builds.remove(0);
        assert_eq!(build.status, BuildStatus::Running);
        assert_eq!(build.activity_id, Some(7));
        assert_eq!(build.host.as_deref(), Some("ssh://builder"));
    }

    #[test]
    fn clean_finish_promotes_leftover_planned_nodes() {
        let mut state = MonitorState::default();
        state.apply_line(
            r#"@nix {"action":"msg","level":3,"msg":"this derivation will be built:"}"#,
        );
        state.apply_line(r#"@nix {"action":"msg","level":3,"msg":"  /nix/store/aaa-app.drv"}"#);

        state.finish(Some(0));
        assert_eq!(
            state.snapshot().builds[0].status,
            BuildStatus::Succeeded,
            "a clean run realised every planned node"
        );
    }

    #[test]
    fn unchanged_closure_does_not_rebroadcast_dependencies() {
        let mut state = MonitorState::default();
        for (id, drv) in [
            (1u64, "/nix/store/aaa-app.drv"),
            (2, "/nix/store/bbb-lib.drv"),
        ] {
            state.apply_line(&format!(
                r#"@nix {{"action":"start","fields":["{drv}","local",1,1],"id":{id},"level":3,"text":"building","type":105}}"#
            ));
        }
        state.drain_deltas();

        let lib = BTreeSet::from(["/nix/store/bbb-lib.drv".to_owned()]);
        state.record_closure("/nix/store/aaa-app.drv".to_owned(), lib.clone());
        let first = count_dependency_deltas(&state.drain_deltas());
        state.record_closure("/nix/store/aaa-app.drv".to_owned(), lib);
        let second = count_dependency_deltas(&state.drain_deltas());

        assert_eq!(first, 1, "the first closure changes the DAG and broadcasts");
        assert_eq!(second, 0, "an identical closure broadcasts nothing");
    }

    fn count_dependency_deltas(deltas: &[Delta]) -> usize {
        deltas
            .iter()
            .filter(|delta| matches!(delta, Delta::DependenciesSet { .. }))
            .count()
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

    #[test]
    fn file_linked_accumulates_optimise_stats() {
        let mut state = MonitorState::default();
        // Nix emits one resFileLinked (type 100) per hard-linked file, fields
        // [apparent_size_bytes, st_blocks].
        state.apply_line(r#"@nix {"action":"result","fields":[4096,8],"id":1,"type":100}"#);
        state.apply_line(r#"@nix {"action":"result","fields":[1024,2],"id":1,"type":100}"#);

        let optimise = state.snapshot().optimise;
        assert_eq!(optimise.files_linked, 2);
        assert_eq!(optimise.bytes_freed, 5120);
    }

    #[test]
    fn file_linked_parses_with_or_without_block_count() {
        // Off Windows Nix sends [bytes, blocks]; on Windows just [bytes].
        let two = parse_line(r#"@nix {"action":"result","fields":[4096,8],"id":1,"type":100}"#);
        assert!(matches!(
            two,
            ParsedLine::Event(NixEvent::Result(ResultAction {
                result: ActivityResult::FileLinked {
                    bytes: 4096,
                    blocks: Some(8)
                },
                ..
            }))
        ));
        let one = parse_line(r#"@nix {"action":"result","fields":[4096],"id":1,"type":100}"#);
        assert!(matches!(
            one,
            ParsedLine::Event(NixEvent::Result(ResultAction {
                result: ActivityResult::FileLinked {
                    bytes: 4096,
                    blocks: None
                },
                ..
            }))
        ));
    }

    #[test]
    fn file_linked_emits_optimise_delta() {
        let mut state = MonitorState::default();
        state.apply_line(r#"@nix {"action":"result","fields":[2048,4],"id":1,"type":100}"#);
        let deltas = state.drain_deltas();
        assert!(
            deltas.iter().any(|delta| matches!(
                delta,
                Delta::OptimiseSet {
                    optimise: OptimiseStats {
                        files_linked: 1,
                        bytes_freed: 2048,
                    }
                }
            )),
            "a hard-linked file should broadcast the updated optimise totals"
        );
    }

    #[test]
    fn copy_to_store_source_handles_both_quote_styles() {
        // `git+file` flakes single-quote and append a trailing slash; `path:`
        // flakes double-quote with no slash. Both must yield the bare path.
        assert_eq!(
            copy_to_store_source("copying '/home/me/proj/' to the store"),
            Some("/home/me/proj")
        );
        assert_eq!(
            copy_to_store_source(r#"copying "/tmp/proj" to the store"#),
            Some("/tmp/proj")
        );
    }

    #[test]
    fn copy_to_store_source_rejects_other_text() {
        assert_eq!(copy_to_store_source("building '/nix/store/x.drv'"), None);
        assert_eq!(copy_to_store_source("copying '/tmp/x' to somewhere else"), None);
        assert_eq!(copy_to_store_source("copying /tmp/x to the store"), None);
        // An empty path would make the server walk its own CWD; reject it.
        assert_eq!(copy_to_store_source("copying '' to the store"), None);
        // Nix's virtual flake-input accessor path has no on-disk location, so
        // it must be rejected rather than walked (one ENOENT per patch file).
        assert_eq!(
            copy_to_store_source(
                "copying '«github:NixOS/nixpkgs#abc»/pkgs/foo/bar.patch' to the store"
            ),
            None
        );
    }

    #[test]
    fn set_activity_size_attaches_bytes_and_reupserts() {
        let mut state = MonitorState::default();
        state.apply_line(
            r#"@nix {"action":"start","id":3,"level":4,"text":"copying \"/tmp/proj\" to the store","type":0}"#,
        );
        state.drain_deltas();

        state.set_activity_size(3, 4_096);

        assert_eq!(state.activities[&3].size_bytes, Some(4_096));
        let upserted = state.drain_deltas().into_iter().find_map(|delta| match delta {
            Delta::ActivityUpsert { activity } if activity.id == 3 => activity.size_bytes,
            _ => None,
        });
        assert_eq!(upserted, Some(4_096), "the measured size rides an ActivityUpsert");
    }

    #[test]
    fn set_activity_size_is_a_noop_for_a_missing_activity() {
        let mut state = MonitorState::default();
        state.set_activity_size(99, 1);
        assert!(
            state.drain_deltas().is_empty(),
            "measuring a dropped activity broadcasts nothing"
        );
    }
}
