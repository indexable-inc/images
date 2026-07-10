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
    /// Active build/substitution goals on the machine, as last polled.
    pub builds: Vec<GlobalBuild>,
    /// Human state line, like [`DaemonInfo.status`](crate::DaemonInfo::status):
    /// the availability note, the active count, or an error.
    pub status: String,
}

impl Default for GlobalBuilds {
    fn default() -> Self {
        Self {
            detected: false,
            builds: Vec::new(),
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
        assert!(!default.status.is_empty());
    }
}
