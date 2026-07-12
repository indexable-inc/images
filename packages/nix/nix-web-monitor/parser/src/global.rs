//! Machine-wide build view (all active `nix` builds on the host).
//!
//! The rest of the monitor watches one `nix` invocation: its own build tree and
//! the daemon syscalls that invocation drives. But a machine can be building for
//! many reasons at once (a CI job, an editor's `nix develop`, another operator's
//! switch), and none of that shows up in a single invocation's tree. This module
//! owns the wire types for a *global* view fed by a patched-nix subcommand,
//! `nix store builds --json`, which reads a daemon-independent status directory
//! and lists every active build/substitution goal on the host, with the
//! why-chain (root derivation -> ... -> this goal) and the cause that forced it.
//!
//! The patched nix keeps one `graph-<pid>.json` per coordinating process, and
//! `nix store builds --graph --json` dumps those documents verbatim: every goal
//! the coordinator knows (waiting, running, done, failed) with its dependency
//! edges. The probe prefers that graph view and derives the flat build list
//! from it via [`flatten_graph`], the same flattening the patched nix itself
//! applies for plain `--json`; on a graph-capable nix the flat subcommand is
//! never polled. An older patched nix without `--graph` still answers the flat
//! form, so both parsers live here.
//!
//! The subcommand is only present on a patched nix, so the whole view degrades
//! gracefully: on stock nix the probe cannot parse a build list, marks the view
//! undetected, and the UI hides the panel. The server owns polling the
//! subcommand; this module owns the (pure, testable) wire types and the
//! defensive JSON parse, so a minor schema drift on the C++ side yields `None`
//! for the affected optional rather than crashing the probe.

use serde::{Deserialize, Serialize};

/// Why one build is happening.
///
/// The chain from the root derivation the operator asked for down to this goal,
/// plus the cause that forced it. Every field is optional so a schema that omits
/// one (or an entry with no known root) still deserializes; the UI renders
/// whatever is present.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GlobalWhy {
    /// The derivation at the top of the want-chain: what was originally
    /// requested (a `nix build .#app`), which this goal is a dependency of.
    /// `None` when the source could not attribute a root.
    pub root_drv_path: Option<String>,
    /// Ordered chain of derivation paths from the root down to this goal, so the
    /// UI can render `app -> ... -> foo`. May be empty.
    pub chain: Vec<String>,
    /// Why nix scheduled this goal: `requested`, `outputsMissing`,
    /// `substitutionFailed`, `outputInvalid`, ... Left as a free string so a new
    /// cause from the C++ side is surfaced verbatim rather than dropped.
    pub cause: Option<String>,
}

/// The kind of goal: a local build or a substitution (fetch from a cache).
///
/// `#[serde(other)]` on [`Other`](GlobalBuildKind::Other) keeps an unknown kind
/// from failing the whole parse; the UI shows it as a neutral badge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GlobalBuildKind {
    /// A derivation being built locally.
    #[default]
    Build,
    /// A path being substituted (downloaded) from a binary cache.
    Substitution,
    /// A kind this build of the monitor does not know; surfaced, not dropped.
    #[serde(other)]
    Other,
}

/// Lifecycle state of one goal in a coordinator's graph.
///
/// `waiting` and `running` are live goals; `done` and `failed` are the
/// session-local record of goals that already completed (the coordinator keeps
/// them so the forest shows finished work draining, not vanishing).
/// `#[serde(other)]` keeps an unknown future state from failing the parse.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GlobalGoalStatus {
    /// Scheduled, with unfinished dependencies (or queued for a build slot).
    #[default]
    Waiting,
    /// Actively building or downloading right now.
    Running,
    /// Completed successfully this session.
    Done,
    /// Completed unsuccessfully this session.
    Failed,
    /// A status this build of the monitor does not know; surfaced, not dropped.
    #[serde(other)]
    Other,
}

/// One goal in a coordinator's graph, as recorded in `graph-<pid>.json` and
/// reported by `nix store builds --graph --json`.
///
/// `id` is the derivation path for a build and the store path for a
/// substitution; `waiters` names the goal ids that *want* this goal (edges
/// point upward, so inverting them yields the dependency forest). The running
/// extras (`start_time`, `log_file`, `builder_pid`) are null on any goal that
/// is not running. Tolerant like the flat row: unknown fields are ignored and
/// missing ones default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GlobalGoal {
    /// The goal's stable identity: drv path (build) or store path (substitution).
    /// Empty when the source omitted it; such a goal is dropped from the forest.
    pub id: String,
    /// Build or substitution.
    pub kind: GlobalBuildKind,
    /// Where the goal is in its lifecycle.
    pub status: GlobalGoalStatus,
    /// Ids of the goals that want this one (dependents, edges pointing up).
    pub waiters: Vec<String>,
    /// Wanted outputs (`out`, `dev`, ...). May be empty.
    pub outputs: Vec<String>,
    /// Unix epoch seconds the goal started running. `None` unless running.
    pub start_time: Option<i64>,
    /// The build log path, once a running build opened one.
    pub log_file: Option<String>,
    /// The builder child pid of a running local build.
    pub builder_pid: Option<i64>,
}

/// One coordinating nix process and everything it is working on: the whole
/// goal graph behind one `graph-<pid>.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GlobalCoordinator {
    /// The coordinator process (the `nix build` / daemon worker), whose
    /// liveness scopes the file: the reader prunes files of dead pids.
    pub pid: Option<i64>,
    /// The client user that requested the work, when attributable.
    pub user: Option<String>,
    /// The client uid, when attributable.
    pub uid: Option<i64>,
    /// Goal ids the client asked for directly: the roots of the forest.
    pub roots: Vec<String>,
    /// Every goal this coordinator knows, live and session-completed.
    pub goals: Vec<GlobalGoal>,
}

/// One active build or substitution goal on the machine.
///
/// As reported by `nix store builds --json`.
/// Fields are optional/defaulted because the exact schema is finalized on the
/// C++ side in parallel: a substitution has no `drvPath` (it sets `storePath`),
/// entries may omit `user`/`uid`/`logFile`, and `outputs` may be empty. Parsing
/// stays lenient (unknown JSON fields ignored, missing optionals -> `None`) so a
/// minor drift does not crash the probe.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GlobalBuild {
    /// The derivation being built. `None` for a substitution, which names a
    /// `store_path` instead.
    pub drv_path: Option<String>,
    /// The store path being substituted. `None` for a local build.
    pub store_path: Option<String>,
    /// Wanted outputs (`out`, `dev`, ...). May be empty.
    pub outputs: Vec<String>,
    /// Build or substitution.
    #[serde(rename = "type")]
    pub kind: GlobalBuildKind,
    /// The worker/builder pid, when the source reported one.
    pub pid: Option<i64>,
    /// Unix epoch *seconds* the goal started, for a live elapsed readout. `None`
    /// when unknown. (Seconds, unlike the rest of the monitor's millisecond
    /// timestamps -- the UI multiplies by 1000 before diffing against its clock.)
    pub start_time: Option<i64>,
    /// The client user that requested the build, when attributable.
    pub user: Option<String>,
    /// The client uid, when attributable.
    pub uid: Option<i64>,
    /// The build log file for this goal (a `.drv.bz2` under the nix log dir),
    /// when the source recorded one. The server may stream it on request.
    pub log_file: Option<String>,
    /// The want-chain and cause that scheduled this goal.
    pub why: GlobalWhy,
}

/// Wire-friendly snapshot of the machine-wide build view.
///
/// Mirrors [`DaemonInfo`](crate::DaemonInfo): `detected` is the analog of
/// `tracing` (false when the subcommand is unavailable, i.e. stock nix), and
/// `status` is the human line the UI shows ("not available (stock nix)",
/// "12 active", or an error). The default is the undetected state, so a fresh
/// `MonitorState` carries an empty view the UI hides until the probe flips it on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalBuilds {
    /// Whether `nix store builds --json` is available and produced a build list.
    /// False on stock nix (no such subcommand); the UI hides the panel.
    pub detected: bool,
    /// Active build/substitution goals on the machine, as last polled. Always
    /// populated: derived from `coordinators` via [`flatten_graph`] when the
    /// graph view is available, polled flat otherwise.
    pub builds: Vec<GlobalBuild>,
    /// Full per-coordinator goal graphs, from `nix store builds --graph
    /// --json`. Empty on a patched nix that predates `--graph` (the panel then
    /// falls back to the flat rows).
    pub coordinators: Vec<GlobalCoordinator>,
    /// Human state line, like [`DaemonInfo.status`](crate::DaemonInfo::status):
    /// the availability note, the active count, or an error.
    pub status: String,
}

impl Default for GlobalBuilds {
    fn default() -> Self {
        Self {
            detected: false,
            builds: Vec::new(),
            coordinators: Vec::new(),
            status: "not available (stock nix)".to_owned(),
        }
    }
}

/// Parse the JSON array `nix store builds --json` prints into a list of goals.
///
/// Tolerant on purpose: unknown fields are ignored and missing optionals become
/// `None` (see the `#[serde(default)]` on the row types), so a schema drift on
/// the C++ side degrades one field rather than failing the whole probe.
///
/// # Errors
///
/// Returns a [`serde_json::Error`] when the payload is not a JSON array of the
/// expected shape. The server treats that as "not detected": stock nix prints an
/// "unknown command" error, not a build array, on the first poll.
pub fn parse_builds(json: &str) -> Result<Vec<GlobalBuild>, serde_json::Error> {
    serde_json::from_str(json)
}

/// The document `nix store builds --graph --json` prints: coordinators wrapped
/// in one object, so the top level can grow siblings without breaking readers.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct GraphDocument {
    coordinators: Vec<GlobalCoordinator>,
}

/// Parse the JSON object `nix store builds --graph --json` prints into the
/// per-coordinator goal graphs. Tolerant like [`parse_builds`].
///
/// # Errors
///
/// Returns a [`serde_json::Error`] when the payload is not an object of the
/// expected shape. The server treats that as "no graph support": a patched nix
/// without `--graph` prints a flag error, not a JSON object, and the probe
/// falls back to the flat form.
pub fn parse_graph(json: &str) -> Result<Vec<GlobalCoordinator>, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    // Require the wrapper object explicitly: serde would happily read a struct
    // from a JSON *array* (fields in order), so a flat `[]` payload would
    // otherwise pass as "no coordinators" and mask a probe wiring bug.
    if !value.is_object() {
        return Err(serde::de::Error::custom("graph payload is not a JSON object"));
    }
    let document: GraphDocument = serde_json::from_value(value)?;
    Ok(document.coordinators)
}

/// Derive the flat running-goal list from the coordinator graphs.
///
/// Mirrors the flattening the patched nix itself applies for plain
/// `nix store builds --json` (`flattenCoordinator` in `store-builds.cc`), so a
/// graph-capable nix is polled once and both views stay consistent: only
/// `running` goals appear, and each row's why-chain is recomputed by walking
/// `waiters` edges up to a root.
pub fn flatten_graph(coordinators: &[GlobalCoordinator]) -> Vec<GlobalBuild> {
    coordinators.iter().flat_map(flatten_coordinator).collect()
}

fn flatten_coordinator(coordinator: &GlobalCoordinator) -> Vec<GlobalBuild> {
    let goals_by_id: std::collections::HashMap<&str, &GlobalGoal> = coordinator
        .goals
        .iter()
        .filter(|goal| !goal.id.is_empty())
        .map(|goal| (goal.id.as_str(), goal))
        .collect();

    coordinator
        .goals
        .iter()
        .filter(|goal| goal.status == GlobalGoalStatus::Running && !goal.id.is_empty())
        .map(|goal| {
            let substitution = goal.kind == GlobalBuildKind::Substitution;
            let chain = why_chain(&goals_by_id, &goal.id);
            // A chain of one is the goal itself: a root the client asked for.
            // Deeper chains carry the same fixed causes the C++ flattening
            // emits (nix does not record the scheduler's actual reason in the
            // graph; these are the two reasons a non-root goal exists).
            let cause = if chain.len() == 1 {
                "requested"
            } else if substitution {
                "outputInvalid"
            } else {
                "outputsMissing"
            };
            GlobalBuild {
                drv_path: (!substitution).then(|| goal.id.clone()),
                store_path: substitution.then(|| goal.id.clone()),
                outputs: goal.outputs.clone(),
                kind: goal.kind,
                pid: coordinator.pid,
                start_time: goal.start_time,
                user: coordinator.user.clone(),
                uid: coordinator.uid,
                log_file: goal.log_file.clone(),
                why: GlobalWhy {
                    root_drv_path: chain.first().cloned(),
                    chain,
                    cause: Some(cause.to_owned()),
                },
            }
        })
        .collect()
}

/// Walk `waiters` edges from `id` up to a root, returned root-first and ending
/// with `id` itself. Any waiter leads to a root, so following the first is
/// enough for a why-chain; the visited set guards against a cyclic document.
fn why_chain(
    goals_by_id: &std::collections::HashMap<&str, &GlobalGoal>,
    id: &str,
) -> Vec<String> {
    let mut chain_leaf_first = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut current = id;
    while visited.insert(current) {
        chain_leaf_first.push(current.to_owned());
        let Some(first_waiter) = goals_by_id
            .get(current)
            .and_then(|goal| goal.waiters.first())
        else {
            break;
        };
        current = first_waiter;
    }
    chain_leaf_first.reverse();
    chain_leaf_first
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A goal the client asked for directly: the writer records
    /// `cause: "requested"` and a why-chain containing only the goal itself
    /// (`isTopGoal` in the C++ writer). The UI keys "requested directly" off
    /// this shape, so it must parse with the root equal to the leaf.
    #[test]
    fn parses_requested_root_whose_chain_is_itself() {
        let json = r#"[
            {
                "drvPath": "/nix/store/aaa-app.drv",
                "storePath": null,
                "outputs": ["out"],
                "type": "build",
                "pid": 77,
                "startTime": 1720200000,
                "user": "alice",
                "uid": 1000,
                "logFile": "/nix/var/log/nix/drvs/aa/a-app.drv.bz2",
                "why": {
                    "rootDrvPath": "/nix/store/aaa-app.drv",
                    "chain": ["/nix/store/aaa-app.drv"],
                    "cause": "requested"
                }
            }
        ]"#;
        let builds = parse_builds(json).expect("requested root parses");
        let build = &builds[0];
        assert_eq!(build.why.cause.as_deref(), Some("requested"));
        assert_eq!(build.why.chain, vec!["/nix/store/aaa-app.drv".to_owned()]);
        assert_eq!(build.why.root_drv_path, build.drv_path);
    }

    #[test]
    fn parses_full_build_with_why_chain() {
        let json = r#"[
            {
                "drvPath": "/nix/store/aaa-foo.drv",
                "storePath": null,
                "outputs": ["out", "dev"],
                "type": "build",
                "pid": 12345,
                "startTime": 1720200000,
                "user": "alice",
                "uid": 1000,
                "logFile": "/nix/var/log/nix/drvs/ab/cdfoo.drv.bz2",
                "why": {
                    "rootDrvPath": "/nix/store/root-app.drv",
                    "chain": ["/nix/store/root-app.drv", "/nix/store/aaa-foo.drv"],
                    "cause": "outputsMissing"
                }
            }
        ]"#;
        let builds = parse_builds(json).expect("valid array parses");
        assert_eq!(builds.len(), 1);
        let build = &builds[0];
        assert_eq!(build.drv_path.as_deref(), Some("/nix/store/aaa-foo.drv"));
        assert_eq!(build.store_path, None);
        assert_eq!(build.outputs, vec!["out".to_owned(), "dev".to_owned()]);
        assert_eq!(build.kind, GlobalBuildKind::Build);
        assert_eq!(build.pid, Some(12345));
        assert_eq!(build.start_time, Some(1_720_200_000));
        assert_eq!(build.user.as_deref(), Some("alice"));
        assert_eq!(build.uid, Some(1000));
        assert_eq!(
            build.log_file.as_deref(),
            Some("/nix/var/log/nix/drvs/ab/cdfoo.drv.bz2")
        );
        assert_eq!(
            build.why.root_drv_path.as_deref(),
            Some("/nix/store/root-app.drv")
        );
        assert_eq!(build.why.chain.len(), 2);
        assert_eq!(build.why.cause.as_deref(), Some("outputsMissing"));
    }

    #[test]
    fn parses_substitution_with_null_drv_path() {
        let json = r#"[
            {
                "drvPath": null,
                "storePath": "/nix/store/bbb-bar",
                "outputs": [],
                "type": "substitution",
                "pid": 999,
                "startTime": 1720200100,
                "user": null,
                "uid": null,
                "logFile": null,
                "why": {
                    "rootDrvPath": null,
                    "chain": [],
                    "cause": "outputInvalid"
                }
            }
        ]"#;
        let builds = parse_builds(json).expect("valid array parses");
        assert_eq!(builds.len(), 1);
        let build = &builds[0];
        assert_eq!(build.drv_path, None);
        assert_eq!(build.store_path.as_deref(), Some("/nix/store/bbb-bar"));
        assert!(build.outputs.is_empty());
        assert_eq!(build.kind, GlobalBuildKind::Substitution);
        assert_eq!(build.user, None);
        assert_eq!(build.uid, None);
        assert_eq!(build.log_file, None);
        assert_eq!(build.why.root_drv_path, None);
        assert!(build.why.chain.is_empty());
        assert_eq!(build.why.cause.as_deref(), Some("outputInvalid"));
    }

    #[test]
    fn parses_entry_missing_optional_fields() {
        // A minimal entry: only the kind is present. Everything else defaults,
        // proving the parse tolerates a source that omits fields entirely
        // (rather than emitting explicit nulls).
        let json = r#"[ { "type": "build" } ]"#;
        let builds = parse_builds(json).expect("minimal entry parses");
        assert_eq!(builds.len(), 1);
        let build = &builds[0];
        assert_eq!(build.kind, GlobalBuildKind::Build);
        assert_eq!(build.drv_path, None);
        assert_eq!(build.store_path, None);
        assert!(build.outputs.is_empty());
        assert_eq!(build.pid, None);
        assert_eq!(build.start_time, None);
        assert_eq!(build.why, GlobalWhy::default());
    }

    #[test]
    fn unknown_kind_and_extra_fields_do_not_fail() {
        // A future kind and an unknown top-level field must not break the parse:
        // the kind falls back to `Other`, the extra field is ignored.
        let json = r#"[
            { "type": "coordinator", "someFutureField": 42, "outputs": ["out"] }
        ]"#;
        let builds = parse_builds(json).expect("tolerant of drift");
        assert_eq!(builds.len(), 1);
        assert_eq!(builds[0].kind, GlobalBuildKind::Other);
        assert_eq!(builds[0].outputs, vec!["out".to_owned()]);
    }

    #[test]
    fn empty_array_is_no_builds() {
        assert!(parse_builds("[]").expect("empty array parses").is_empty());
    }

    #[test]
    fn non_array_payload_errors() {
        // Stock nix's "unknown subcommand" text is not a JSON array; the server
        // relies on this being an error to mark the view undetected.
        assert!(parse_builds("error: unknown flag").is_err());
    }

    #[test]
    fn default_is_undetected() {
        let default = GlobalBuilds::default();
        assert!(!default.detected);
        assert!(default.builds.is_empty());
        assert!(default.coordinators.is_empty());
        assert!(!default.status.is_empty());
    }

    /// A representative graph document: one coordinator building a root whose
    /// dependency is running while the root waits, plus a substitution that
    /// already finished. Field-for-field what the patched nix emits.
    const GRAPH_FIXTURE: &str = r#"{
        "coordinators": [
            {
                "pid": 4242,
                "user": "alice",
                "uid": 1000,
                "roots": ["/nix/store/aaa-app.drv"],
                "goals": [
                    {
                        "id": "/nix/store/aaa-app.drv",
                        "kind": "build",
                        "status": "waiting",
                        "waiters": [],
                        "outputs": ["out"],
                        "startTime": null,
                        "logFile": null,
                        "builderPid": null
                    },
                    {
                        "id": "/nix/store/bbb-dep.drv",
                        "kind": "build",
                        "status": "running",
                        "waiters": ["/nix/store/aaa-app.drv"],
                        "outputs": ["out"],
                        "startTime": 1720200000,
                        "logFile": "/nix/var/log/nix/drvs/bb/b-dep.drv.bz2",
                        "builderPid": 777
                    },
                    {
                        "id": "/nix/store/ccc-src",
                        "kind": "substitution",
                        "status": "done",
                        "waiters": ["/nix/store/bbb-dep.drv"],
                        "outputs": [],
                        "startTime": null,
                        "logFile": null,
                        "builderPid": null
                    }
                ]
            }
        ]
    }"#;

    #[test]
    fn parses_graph_document() {
        let coordinators = parse_graph(GRAPH_FIXTURE).expect("graph document parses");
        assert_eq!(coordinators.len(), 1);
        let coordinator = &coordinators[0];
        assert_eq!(coordinator.pid, Some(4242));
        assert_eq!(coordinator.user.as_deref(), Some("alice"));
        assert_eq!(coordinator.roots, vec!["/nix/store/aaa-app.drv".to_owned()]);
        assert_eq!(coordinator.goals.len(), 3);

        let root = &coordinator.goals[0];
        assert_eq!(root.status, GlobalGoalStatus::Waiting);
        assert!(root.waiters.is_empty());
        assert_eq!(root.start_time, None);

        let dep = &coordinator.goals[1];
        assert_eq!(dep.status, GlobalGoalStatus::Running);
        assert_eq!(dep.waiters, vec!["/nix/store/aaa-app.drv".to_owned()]);
        assert_eq!(dep.start_time, Some(1_720_200_000));
        assert_eq!(dep.builder_pid, Some(777));

        let src = &coordinator.goals[2];
        assert_eq!(src.kind, GlobalBuildKind::Substitution);
        assert_eq!(src.status, GlobalGoalStatus::Done);
    }

    /// The flat derivation of the graph: only the running dep appears, with the
    /// coordinator's identity and a why-chain recomputed from `waiters`. This
    /// mirrors `flattenCoordinator` in the patched nix, so the flat rows look
    /// the same whichever side flattened.
    #[test]
    fn flatten_graph_yields_running_goals_with_why_chain() {
        let coordinators = parse_graph(GRAPH_FIXTURE).expect("graph document parses");
        let builds = flatten_graph(&coordinators);
        assert_eq!(builds.len(), 1, "only the running goal flattens");
        let build = &builds[0];
        assert_eq!(build.drv_path.as_deref(), Some("/nix/store/bbb-dep.drv"));
        assert_eq!(build.kind, GlobalBuildKind::Build);
        assert_eq!(build.pid, Some(4242));
        assert_eq!(build.user.as_deref(), Some("alice"));
        assert_eq!(build.uid, Some(1000));
        assert_eq!(build.start_time, Some(1_720_200_000));
        assert_eq!(
            build.log_file.as_deref(),
            Some("/nix/var/log/nix/drvs/bb/b-dep.drv.bz2")
        );
        assert_eq!(
            build.why.chain,
            vec![
                "/nix/store/aaa-app.drv".to_owned(),
                "/nix/store/bbb-dep.drv".to_owned()
            ]
        );
        assert_eq!(
            build.why.root_drv_path.as_deref(),
            Some("/nix/store/aaa-app.drv")
        );
        assert_eq!(build.why.cause.as_deref(), Some("outputsMissing"));
    }

    /// A running root (nothing above it) flattens to `requested`, and a running
    /// substitution names `storePath` with cause `outputInvalid`: the exact
    /// labels the C++ flattening emits.
    #[test]
    fn flatten_graph_labels_roots_and_substitutions() {
        let json = r#"{
            "coordinators": [
                {
                    "pid": 1,
                    "roots": ["/nix/store/aaa-app.drv"],
                    "goals": [
                        {
                            "id": "/nix/store/aaa-app.drv",
                            "kind": "build",
                            "status": "running",
                            "waiters": [],
                            "startTime": 1720200000
                        },
                        {
                            "id": "/nix/store/bbb-src",
                            "kind": "substitution",
                            "status": "running",
                            "waiters": ["/nix/store/aaa-app.drv"]
                        }
                    ]
                }
            ]
        }"#;
        let builds = flatten_graph(&parse_graph(json).expect("graph parses"));
        assert_eq!(builds.len(), 2);
        assert_eq!(builds[0].why.cause.as_deref(), Some("requested"));
        assert_eq!(builds[0].why.chain.len(), 1);
        assert_eq!(builds[1].store_path.as_deref(), Some("/nix/store/bbb-src"));
        assert_eq!(builds[1].drv_path, None);
        assert_eq!(builds[1].why.cause.as_deref(), Some("outputInvalid"));
        assert_eq!(
            builds[1].why.root_drv_path.as_deref(),
            Some("/nix/store/aaa-app.drv")
        );
    }

    /// A cyclic `waiters` document (which a healthy nix never writes) must not
    /// hang the chain walk: the visited guard cuts the loop.
    #[test]
    fn flatten_graph_survives_waiter_cycles() {
        let json = r#"{
            "coordinators": [
                {
                    "pid": 1,
                    "goals": [
                        { "id": "a", "status": "running", "waiters": ["b"] },
                        { "id": "b", "status": "waiting", "waiters": ["a"] }
                    ]
                }
            ]
        }"#;
        let builds = flatten_graph(&parse_graph(json).expect("graph parses"));
        assert_eq!(builds.len(), 1);
        assert_eq!(builds[0].why.chain, vec!["b".to_owned(), "a".to_owned()]);
    }

    /// A goal with an unknown status or a missing id degrades quietly: the
    /// status parses as `Other` and the id-less goal never flattens.
    #[test]
    fn graph_tolerates_unknown_status_and_missing_id() {
        let json = r#"{
            "coordinators": [
                {
                    "goals": [
                        { "id": "a", "status": "paused" },
                        { "status": "running" }
                    ]
                }
            ]
        }"#;
        let coordinators = parse_graph(json).expect("graph parses");
        assert_eq!(coordinators[0].goals[0].status, GlobalGoalStatus::Other);
        assert!(flatten_graph(&coordinators).is_empty());
    }

    /// A patched nix without `--graph` prints a flag error, not a JSON object;
    /// the server relies on the parse error to fall back to the flat form.
    #[test]
    fn non_object_graph_payload_errors() {
        assert!(parse_graph("error: unrecognised flag '--graph'").is_err());
        assert!(parse_graph("[]").is_err());
    }
}
