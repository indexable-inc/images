use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use serde::Deserialize;

#[derive(Deserialize)]
struct Entry {
    key: String,
    value: String,
}

/// A flag block injected only when the user did not already pass an equivalent
/// option: withheld if any user arg (scanning until the first `--`) equals a
/// name in `unless_present` or starts with `"<name>="`.
#[derive(Deserialize)]
struct ConditionalFlags {
    unless_present: Vec<String>,
    flags: Vec<String>,
}

/// The launch spec, read as JSON from `IX_LAUNCH_SPEC`. Every field beyond
/// `target` is optional so each consumer uses only the layers it needs: codex
/// sets the `forced`/`soft` `--config` layer; claude-code sets
/// `env`/`env_defaults`/`path_prepend`/`flags`/`conditional_flags`.
#[derive(Deserialize)]
struct Spec {
    /// Real binary to exec.
    target: String,

    // --- codex `--config` layer (forced always; soft only when the dotted key
    // is absent from the target's config file) ---
    #[serde(default)]
    config_dir_env: String,
    #[serde(default)]
    config_dir_default: String,
    #[serde(default)]
    config_file: String,
    #[serde(default)]
    forced: Vec<Entry>,
    #[serde(default)]
    soft: Vec<Entry>,

    // --- generic launcher layers ---
    /// Environment variables set unconditionally.
    #[serde(default)]
    env: Vec<Entry>,
    /// Environment variables set only when not already present in the caller's
    /// environment (the old `export NAME="${NAME-default}"`).
    #[serde(default)]
    env_defaults: Vec<Entry>,
    /// Directories prepended to `PATH` (ahead of the caller's PATH).
    #[serde(default)]
    path_prepend: Vec<String>,
    /// Flags prepended before the user argv, unconditionally.
    #[serde(default)]
    flags: Vec<String>,
    /// Flag blocks prepended only when the user passed no equivalent option.
    #[serde(default)]
    conditional_flags: Vec<ConditionalFlags>,
}

fn expand_tilde(path: &str) -> String {
    match (path.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => path.to_owned(),
    }
}

fn config_path(spec: &Spec) -> PathBuf {
    let dir = std::env::var(&spec.config_dir_env)
        .unwrap_or_else(|_| expand_tilde(&spec.config_dir_default));
    PathBuf::from(dir).join(&spec.config_file)
}

/// Returns true if `dotted` key-path is present anywhere in the TOML value.
fn is_set(cfg: &toml::Value, dotted: &str) -> bool {
    let mut cur = cfg;
    for seg in dotted.split('.') {
        match cur.get(seg) {
            Some(next) => cur = next,
            None => return false,
        }
    }
    true
}

/// The codex `--config k=v` layer: forced always, soft only when absent from
/// the parsed config file.
fn build_config_flags(spec: &Spec, cfg: Option<&toml::Value>) -> Vec<String> {
    let mut out = Vec::new();
    for entry in &spec.forced {
        out.push("--config".to_owned());
        out.push(format!("{}={}", entry.key, entry.value));
    }
    for entry in &spec.soft {
        if !cfg.is_some_and(|c| is_set(c, &entry.key)) {
            out.push("--config".to_owned());
            out.push(format!("{}={}", entry.key, entry.value));
        }
    }
    out
}

/// True if any user arg (up to the first `--`) is `name` or `name=...` for some
/// name in `names`.
fn arg_present(user_args: &[OsString], names: &[String]) -> bool {
    for arg in user_args {
        let Some(s) = arg.to_str() else { continue };
        if s == "--" {
            break;
        }
        for name in names {
            if s == name
                || s.strip_prefix(name)
                    .is_some_and(|rest| rest.starts_with('='))
            {
                return true;
            }
        }
    }
    false
}

/// `flags` plus each conditional block whose options the user did not supply.
fn build_arg_flags(spec: &Spec, user_args: &[OsString]) -> Vec<String> {
    let mut out = spec.flags.clone();
    for block in &spec.conditional_flags {
        if !arg_present(user_args, &block.unless_present) {
            out.extend(block.flags.iter().cloned());
        }
    }
    out
}

/// `path_prepend` joined ahead of the current `PATH` (or alone if PATH is unset).
fn build_path(prepend: &[String], current: Option<&str>) -> String {
    let mut p = prepend.join(":");
    if let Some(cur) = current.filter(|c| !c.is_empty()) {
        p.push(':');
        p.push_str(cur);
    }
    p
}

fn load_spec() -> Result<Spec, String> {
    let path = std::env::var("IX_LAUNCH_SPEC").map_err(|_| "IX_LAUNCH_SPEC not set".to_owned())?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("read spec {path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("parse spec {path}: {e}"))
}

fn main() -> ExitCode {
    let spec = match load_spec() {
        Ok(spec) => spec,
        Err(err) => {
            eprintln!("config-launch: {err}");
            return ExitCode::from(78);
        }
    };

    let mut argv = std::env::args_os();
    let argv0 = argv.next().unwrap_or_else(|| spec.target.clone().into());
    let user_args: Vec<OsString> = argv.collect();

    // Read the config file only when there are soft keys whose presence it
    // gates (claude-code sets no soft keys, so it never needs a config dir).
    let cfg = if spec.soft.is_empty() {
        None
    } else {
        std::fs::read_to_string(config_path(&spec))
            .ok()
            .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
    };

    let mut prepended = build_arg_flags(&spec, &user_args);
    prepended.extend(build_config_flags(&spec, cfg.as_ref()));

    let mut cmd = Command::new(&spec.target);
    cmd.arg0(&argv0);
    for entry in &spec.env {
        cmd.env(&entry.key, &entry.value);
    }
    for entry in &spec.env_defaults {
        if std::env::var_os(&entry.key).is_none() {
            cmd.env(&entry.key, &entry.value);
        }
    }
    if !spec.path_prepend.is_empty() {
        let current = std::env::var("PATH").ok();
        cmd.env("PATH", build_path(&spec.path_prepend, current.as_deref()));
    }
    cmd.args(&prepended);
    cmd.args(&user_args);

    let err = cmd.exec();
    eprintln!("config-launch: exec {} failed: {err}", spec.target);
    ExitCode::from(127)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{
        ConditionalFlags, Entry, Spec, arg_present, build_arg_flags, build_config_flags,
        build_path, is_set,
    };

    #[derive(Default)]
    struct SpecBuilder {
        forced: Vec<(&'static str, &'static str)>,
        soft: Vec<(&'static str, &'static str)>,
        flags: Vec<&'static str>,
        conditional: Vec<(Vec<&'static str>, Vec<&'static str>)>,
    }

    fn entries(pairs: Vec<(&str, &str)>) -> Vec<Entry> {
        pairs
            .into_iter()
            .map(|(k, v)| Entry {
                key: k.to_owned(),
                value: v.to_owned(),
            })
            .collect()
    }

    fn make_spec(b: SpecBuilder) -> Spec {
        Spec {
            target: "/bin/stub".to_owned(),
            config_dir_env: "TEST_HOME".to_owned(),
            config_dir_default: "~/.test".to_owned(),
            config_file: "config.toml".to_owned(),
            forced: entries(b.forced),
            soft: entries(b.soft),
            env: Vec::new(),
            env_defaults: Vec::new(),
            path_prepend: Vec::new(),
            flags: b.flags.into_iter().map(str::to_owned).collect(),
            conditional_flags: b
                .conditional
                .into_iter()
                .map(|(unless, flags)| ConditionalFlags {
                    unless_present: unless.into_iter().map(str::to_owned).collect(),
                    flags: flags.into_iter().map(str::to_owned).collect(),
                })
                .collect(),
        }
    }

    fn forced(pairs: Vec<(&'static str, &'static str)>) -> SpecBuilder {
        SpecBuilder {
            forced: pairs,
            ..SpecBuilder::default()
        }
    }

    fn soft(pairs: Vec<(&'static str, &'static str)>) -> SpecBuilder {
        SpecBuilder {
            soft: pairs,
            ..SpecBuilder::default()
        }
    }

    /// True if `flags` contains a `--config <expected>` pair.
    fn has_config(flags: &[String], expected: &str) -> bool {
        flags
            .windows(2)
            .any(|w| w[0] == "--config" && w[1] == expected)
    }

    fn parse_toml(s: &str) -> toml::Value {
        toml::from_str(s).expect("valid toml")
    }

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn forced_always_injected_no_config() {
        let spec = make_spec(forced(vec![("check_for_update_on_startup", "false")]));
        let flags = build_config_flags(&spec, None);
        assert!(
            has_config(&flags, "check_for_update_on_startup=false"),
            "forced flag should always be injected; got: {flags:?}"
        );
    }

    #[test]
    fn forced_always_injected_with_config() {
        let spec = make_spec(forced(vec![("check_for_update_on_startup", "false")]));
        let cfg = parse_toml("check_for_update_on_startup = true\n");
        let flags = build_config_flags(&spec, Some(&cfg));
        assert!(
            has_config(&flags, "check_for_update_on_startup=false"),
            "forced flag must override even when user config sets the key; got: {flags:?}"
        );
    }

    #[test]
    fn soft_injected_when_absent() {
        let spec = make_spec(soft(vec![
            ("features.multi_agent_v2.enabled", "true"),
            (
                "features.multi_agent_v2.max_concurrent_threads_per_session",
                "16",
            ),
            ("agents.max_depth", "3"),
        ]));
        let flags = build_config_flags(&spec, None);
        assert!(
            has_config(&flags, "features.multi_agent_v2.enabled=true"),
            "soft flag should be injected when config absent; got: {flags:?}"
        );
        assert!(
            has_config(
                &flags,
                "features.multi_agent_v2.max_concurrent_threads_per_session=16"
            ),
            "soft flag should be injected when config absent; got: {flags:?}"
        );
        assert!(
            has_config(&flags, "agents.max_depth=3"),
            "soft flag should be injected when config absent; got: {flags:?}"
        );
    }

    #[test]
    fn soft_withheld_when_exact_nested_key_set() {
        // config sets [features.multi_agent_v2] enabled = false; only the
        // `features.multi_agent_v2.enabled` leaf is present, so:
        //   - `features.multi_agent_v2.enabled` is withheld (exact match)
        //   - `features.multi_agent_v2.max_concurrent_threads_per_session` is still
        //     injected (that leaf is absent from config)
        //   - `agents.max_depth` is injected (different subtree)
        let spec = make_spec(soft(vec![
            ("features.multi_agent_v2.enabled", "true"),
            (
                "features.multi_agent_v2.max_concurrent_threads_per_session",
                "16",
            ),
            ("agents.max_depth", "3"),
        ]));
        let cfg = parse_toml("[features.multi_agent_v2]\nenabled = false\n");
        let flags = build_config_flags(&spec, Some(&cfg));

        assert!(
            !has_config(&flags, "features.multi_agent_v2.enabled=true"),
            "enabled soft key should be withheld because config sets it; got: {flags:?}"
        );
        assert!(
            has_config(
                &flags,
                "features.multi_agent_v2.max_concurrent_threads_per_session=16"
            ),
            "threads soft key should be injected because config does not set it; got: {flags:?}"
        );
        assert!(
            has_config(&flags, "agents.max_depth=3"),
            "max_depth should be injected when only v2.enabled is set; got: {flags:?}"
        );
    }

    #[test]
    fn soft_withheld_when_exact_key_set() {
        let spec = make_spec(soft(vec![("agents.max_depth", "3")]));
        let cfg = parse_toml("[agents]\nmax_depth = 5\n");
        let flags = build_config_flags(&spec, Some(&cfg));
        assert!(
            flags.is_empty(),
            "soft key should be withheld when exact path is in config; got: {flags:?}"
        );
    }

    #[test]
    fn is_set_partial_path() {
        let cfg = parse_toml("[features.multi_agent_v2]\nenabled = false\n");
        assert!(is_set(&cfg, "features.multi_agent_v2.enabled"));
        assert!(is_set(&cfg, "features.multi_agent_v2"));
        assert!(is_set(&cfg, "features"));
        assert!(!is_set(&cfg, "features.other"));
        assert!(!is_set(&cfg, "agents.max_depth"));
    }

    #[test]
    fn is_set_top_level_key() {
        let cfg = parse_toml("check_for_update_on_startup = false\n");
        assert!(is_set(&cfg, "check_for_update_on_startup"));
        assert!(!is_set(&cfg, "other"));
    }

    #[test]
    fn no_flags_when_all_soft_set() {
        let spec = make_spec(soft(vec![
            ("features.multi_agent_v2.enabled", "true"),
            (
                "features.multi_agent_v2.max_concurrent_threads_per_session",
                "16",
            ),
            ("agents.max_depth", "3"),
        ]));
        let toml_text = "[features.multi_agent_v2]\nenabled = true\nmax_concurrent_threads_per_session = 8\n[agents]\nmax_depth = 5\n";
        let cfg = parse_toml(toml_text);
        let flags = build_config_flags(&spec, Some(&cfg));
        assert!(
            flags.is_empty(),
            "no soft flags should be injected when all keys present; got: {flags:?}"
        );
    }

    #[test]
    fn static_flags_prepend() {
        let spec = make_spec(SpecBuilder {
            flags: vec!["--debug", "--thinking-display=summarized"],
            ..SpecBuilder::default()
        });
        let flags = build_arg_flags(&spec, &os(&["mcp", "list"]));
        assert_eq!(flags, vec!["--debug", "--thinking-display=summarized"]);
    }

    #[test]
    fn conditional_flag_injected_when_option_absent() {
        let spec = make_spec(SpecBuilder {
            conditional: vec![(vec!["--settings"], vec!["--settings=/def.json"])],
            ..SpecBuilder::default()
        });
        let flags = build_arg_flags(&spec, &os(&["-p", "hi"]));
        assert_eq!(flags, vec!["--settings=/def.json"]);
    }

    #[test]
    fn conditional_flag_withheld_when_bare_option_present() {
        let spec = make_spec(SpecBuilder {
            conditional: vec![(vec!["--settings"], vec!["--settings=/def.json"])],
            ..SpecBuilder::default()
        });
        let flags = build_arg_flags(&spec, &os(&["--settings", "/user.json"]));
        assert!(
            flags.is_empty(),
            "conditional should be withheld when user passes the bare option; got: {flags:?}"
        );
    }

    #[test]
    fn conditional_flag_withheld_when_equals_option_present() {
        let spec = make_spec(SpecBuilder {
            conditional: vec![(vec!["--settings"], vec!["--settings=/def.json"])],
            ..SpecBuilder::default()
        });
        let flags = build_arg_flags(&spec, &os(&["--settings=/user.json"]));
        assert!(
            flags.is_empty(),
            "conditional should be withheld for the `=` form; got: {flags:?}"
        );
    }

    #[test]
    fn arg_present_stops_at_double_dash() {
        // a `--settings` after `--` is a positional, not our option.
        assert!(!arg_present(
            &os(&["--", "--settings", "x"]),
            &["--settings".to_owned()]
        ));
        assert!(arg_present(
            &os(&["--settings", "x"]),
            &["--settings".to_owned()]
        ));
    }

    #[test]
    fn build_path_prepends_ahead_of_current() {
        let p = build_path(
            &["/a/bin".to_owned(), "/b/bin".to_owned()],
            Some("/usr/bin"),
        );
        assert_eq!(p, "/a/bin:/b/bin:/usr/bin");
    }

    #[test]
    fn build_path_without_current() {
        let p = build_path(&["/a/bin".to_owned()], None);
        assert_eq!(p, "/a/bin");
        let p_empty = build_path(&["/a/bin".to_owned()], Some(""));
        assert_eq!(p_empty, "/a/bin");
    }
}
