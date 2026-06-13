use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::process::Command;

use tempfile::TempDir;

/// Path to the compiled config-launch binary, injected by Cargo at test-compile time.
const BIN: &str = env!("CARGO_BIN_EXE_config-launch");

/// A stub target script that prints each argument on its own line to stdout.
fn write_stub(dir: &TempDir) -> std::path::PathBuf {
    let path = dir.path().join("stub");
    let mut f = fs::File::create(&path).expect("create stub");
    f.write_all(b"#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done\n")
        .expect("write stub");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod stub");
    path
}

/// Build a JSON spec and write it to a temp file; return the path.
fn write_spec(
    dir: &TempDir,
    target: &str,
    config_dir_env: &str,
    config_dir_default: &str,
    config_file: &str,
    forced: &[(&str, &str)],
    soft: &[(&str, &str)],
) -> std::path::PathBuf {
    let forced_json: Vec<serde_json::Value> = forced
        .iter()
        .map(|(k, v)| serde_json::json!({ "key": k, "value": v }))
        .collect();
    let soft_json: Vec<serde_json::Value> = soft
        .iter()
        .map(|(k, v)| serde_json::json!({ "key": k, "value": v }))
        .collect();
    let spec = serde_json::json!({
        "target": target,
        "config_dir_env": config_dir_env,
        "config_dir_default": config_dir_default,
        "config_file": config_file,
        "forced": forced_json,
        "soft": soft_json,
    });
    let path = dir.path().join("spec.json");
    fs::write(&path, spec.to_string()).expect("write spec");
    path
}

fn run_launcher(spec_path: &std::path::Path, config_dir: &std::path::Path) -> Vec<String> {
    let output = Command::new(BIN)
        .env("IX_LAUNCH_SPEC", spec_path)
        .env("TEST_CONFIG_DIR", config_dir)
        .output()
        .expect("run config-launch");
    assert!(
        output.status.success(),
        "launcher exited with {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf8 stdout")
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn forced_always_present() {
    let tmp = TempDir::new().unwrap();
    let stub = write_stub(&tmp);
    let cfg_dir = tmp.path().join("cfg");
    fs::create_dir_all(&cfg_dir).unwrap();
    let spec = write_spec(
        &tmp,
        stub.to_str().unwrap(),
        "TEST_CONFIG_DIR",
        "~/.test",
        "config.toml",
        &[("check_for_update_on_startup", "false")],
        &[],
    );
    let lines = run_launcher(&spec, &cfg_dir);
    assert!(
        lines.contains(&"--config".to_owned()),
        "expected --config in output: {lines:?}"
    );
    assert!(
        lines.contains(&"check_for_update_on_startup=false".to_owned()),
        "expected forced kv; got: {lines:?}"
    );
}

#[test]
fn soft_injected_when_absent() {
    let tmp = TempDir::new().unwrap();
    let stub = write_stub(&tmp);
    let cfg_dir = tmp.path().join("cfg");
    fs::create_dir_all(&cfg_dir).unwrap();
    // no config.toml in cfg_dir
    let spec = write_spec(
        &tmp,
        stub.to_str().unwrap(),
        "TEST_CONFIG_DIR",
        "~/.test",
        "config.toml",
        &[],
        &[
            ("features.multi_agent_v2.enabled", "true"),
            ("agents.max_depth", "3"),
        ],
    );
    let lines = run_launcher(&spec, &cfg_dir);
    assert!(
        lines.contains(&"features.multi_agent_v2.enabled=true".to_owned()),
        "soft flag should inject when absent; got: {lines:?}"
    );
    assert!(
        lines.contains(&"agents.max_depth=3".to_owned()),
        "soft flag should inject when absent; got: {lines:?}"
    );
}

#[test]
fn soft_withheld_when_set_in_config() {
    let tmp = TempDir::new().unwrap();
    let stub = write_stub(&tmp);
    let cfg_dir = tmp.path().join("cfg");
    fs::create_dir_all(&cfg_dir).unwrap();
    // write config.toml that sets multi_agent_v2
    fs::write(
        cfg_dir.join("config.toml"),
        "[features.multi_agent_v2]\nenabled = false\n",
    )
    .unwrap();
    let spec = write_spec(
        &tmp,
        stub.to_str().unwrap(),
        "TEST_CONFIG_DIR",
        "~/.test",
        "config.toml",
        &[("check_for_update_on_startup", "false")],
        &[
            ("features.multi_agent_v2.enabled", "true"),
            (
                "features.multi_agent_v2.max_concurrent_threads_per_session",
                "16",
            ),
            ("agents.max_depth", "3"),
        ],
    );
    let lines = run_launcher(&spec, &cfg_dir);

    // forced must still be present
    assert!(
        lines.contains(&"check_for_update_on_startup=false".to_owned()),
        "forced flag must always be present; got: {lines:?}"
    );
    // v2.enabled is set in config -> withheld
    assert!(
        !lines.contains(&"features.multi_agent_v2.enabled=true".to_owned()),
        "v2 enabled soft key should be withheld when user config sets it; got: {lines:?}"
    );
    // max_concurrent_threads_per_session is NOT set in config -> injected
    assert!(
        lines.contains(&"features.multi_agent_v2.max_concurrent_threads_per_session=16".to_owned()),
        "threads soft key should be injected because config does not set it; got: {lines:?}"
    );
    // max_depth is a different path so it should still inject
    assert!(
        lines.contains(&"agents.max_depth=3".to_owned()),
        "max_depth should inject when only v2 is set; got: {lines:?}"
    );
}

#[test]
fn argv_passthrough() {
    let tmp = TempDir::new().unwrap();
    let stub = write_stub(&tmp);
    let cfg_dir = tmp.path().join("cfg");
    fs::create_dir_all(&cfg_dir).unwrap();
    let spec = write_spec(
        &tmp,
        stub.to_str().unwrap(),
        "TEST_CONFIG_DIR",
        "~/.test",
        "config.toml",
        &[],
        &[],
    );
    let output = Command::new(BIN)
        .env("IX_LAUNCH_SPEC", &spec)
        .env("TEST_CONFIG_DIR", &cfg_dir)
        .args(["exec", "hi", "--model", "o3"])
        .output()
        .expect("run");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines.contains(&"exec"), "expected 'exec' in passthrough");
    assert!(lines.contains(&"hi"), "expected 'hi' in passthrough");
    assert!(
        lines.contains(&"--model"),
        "expected '--model' in passthrough"
    );
    assert!(lines.contains(&"o3"), "expected 'o3' in passthrough");
}

#[test]
fn missing_spec_env_exits_78() {
    let output = Command::new(BIN)
        .env_remove("IX_LAUNCH_SPEC")
        .output()
        .expect("run");
    assert_eq!(
        output.status.code(),
        Some(78),
        "should exit 78 when IX_LAUNCH_SPEC unset"
    );
}
