use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use serde::Deserialize;

#[derive(Deserialize)]
struct Entry {
    key: String,
    value: String,
}

#[derive(Deserialize)]
struct Spec {
    target: String,
    config_dir_env: String,
    config_dir_default: String,
    config_file: String,
    forced: Vec<Entry>,
    soft: Vec<Entry>,
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

fn build_flags(spec: &Spec, cfg: Option<&toml::Value>) -> Vec<String> {
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
    let cfg = std::fs::read_to_string(config_path(&spec))
        .ok()
        .and_then(|text| toml::from_str::<toml::Value>(&text).ok());
    let flags = build_flags(&spec, cfg.as_ref());

    let mut args = std::env::args_os();
    let argv0 = args.next().unwrap_or_else(|| spec.target.clone().into());
    let err = Command::new(&spec.target)
        .arg0(&argv0)
        .args(&flags)
        .args(args)
        .exec();
    eprintln!("config-launch: exec {} failed: {err}", spec.target);
    ExitCode::from(127)
}

#[cfg(test)]
mod tests {
    use super::{Entry, Spec, build_flags, is_set};

    fn make_spec(forced: Vec<(&str, &str)>, soft: Vec<(&str, &str)>) -> Spec {
        Spec {
            target: "/bin/stub".to_owned(),
            config_dir_env: "TEST_HOME".to_owned(),
            config_dir_default: "~/.test".to_owned(),
            config_file: "config.toml".to_owned(),
            forced: forced
                .into_iter()
                .map(|(k, v)| Entry {
                    key: k.to_owned(),
                    value: v.to_owned(),
                })
                .collect(),
            soft: soft
                .into_iter()
                .map(|(k, v)| Entry {
                    key: k.to_owned(),
                    value: v.to_owned(),
                })
                .collect(),
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

    #[test]
    fn forced_always_injected_no_config() {
        let spec = make_spec(vec![("check_for_update_on_startup", "false")], vec![]);
        let flags = build_flags(&spec, None);
        assert!(
            has_config(&flags, "check_for_update_on_startup=false"),
            "forced flag should always be injected; got: {flags:?}"
        );
    }

    #[test]
    fn forced_always_injected_with_config() {
        let spec = make_spec(vec![("check_for_update_on_startup", "false")], vec![]);
        let cfg = parse_toml("check_for_update_on_startup = true\n");
        let flags = build_flags(&spec, Some(&cfg));
        assert!(
            has_config(&flags, "check_for_update_on_startup=false"),
            "forced flag must override even when user config sets the key; got: {flags:?}"
        );
    }

    #[test]
    fn soft_injected_when_absent() {
        let spec = make_spec(
            vec![],
            vec![
                ("features.multi_agent_v2.enabled", "true"),
                (
                    "features.multi_agent_v2.max_concurrent_threads_per_session",
                    "16",
                ),
                ("agents.max_depth", "3"),
            ],
        );
        let flags = build_flags(&spec, None);
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
        let spec = make_spec(
            vec![],
            vec![
                ("features.multi_agent_v2.enabled", "true"),
                (
                    "features.multi_agent_v2.max_concurrent_threads_per_session",
                    "16",
                ),
                ("agents.max_depth", "3"),
            ],
        );
        let cfg = parse_toml("[features.multi_agent_v2]\nenabled = false\n");
        let flags = build_flags(&spec, Some(&cfg));

        assert!(
            !has_config(&flags, "features.multi_agent_v2.enabled=true"),
            "enabled soft key should be withheld because config sets it; got: {flags:?}"
        );
        // max_concurrent_threads_per_session is NOT set in config, so it IS injected
        assert!(
            has_config(
                &flags,
                "features.multi_agent_v2.max_concurrent_threads_per_session=16"
            ),
            "threads soft key should be injected because config does not set it; got: {flags:?}"
        );
        // agents.max_depth is a different subtree, should be injected
        assert!(
            has_config(&flags, "agents.max_depth=3"),
            "max_depth should be injected when only v2.enabled is set; got: {flags:?}"
        );
    }

    #[test]
    fn soft_withheld_when_exact_key_set() {
        let spec = make_spec(vec![], vec![("agents.max_depth", "3")]);
        let cfg = parse_toml("[agents]\nmax_depth = 5\n");
        let flags = build_flags(&spec, Some(&cfg));
        assert!(
            flags.is_empty(),
            "soft key should be withheld when exact path is in config; got: {flags:?}"
        );
    }

    #[test]
    fn is_set_partial_path() {
        let cfg = parse_toml("[features.multi_agent_v2]\nenabled = false\n");
        // full path present
        assert!(is_set(&cfg, "features.multi_agent_v2.enabled"));
        // parent path also considered set
        assert!(is_set(&cfg, "features.multi_agent_v2"));
        assert!(is_set(&cfg, "features"));
        // absent path
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
        let spec = make_spec(
            vec![],
            vec![
                ("features.multi_agent_v2.enabled", "true"),
                (
                    "features.multi_agent_v2.max_concurrent_threads_per_session",
                    "16",
                ),
                ("agents.max_depth", "3"),
            ],
        );
        let toml_text = "[features.multi_agent_v2]\nenabled = true\nmax_concurrent_threads_per_session = 8\n[agents]\nmax_depth = 5\n";
        let cfg = parse_toml(toml_text);
        let flags = build_flags(&spec, Some(&cfg));
        assert!(
            flags.is_empty(),
            "no soft flags should be injected when all keys present; got: {flags:?}"
        );
    }
}
