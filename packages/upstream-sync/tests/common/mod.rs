#![allow(
    dead_code,
    reason = "each integration-test binary compiles this module separately and uses a subset"
)]

//! Shared harness for the binary integration tests: stub tools on PATH and a
//! scratch repo layout mirroring the nix `runCommand` tests this crate's
//! predecessor shipped with.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Write an executable `#!/bin/sh` stub named `name` into `dir`.
pub fn write_stub(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// PATH with `stub_dir` prepended.
pub fn stub_path(stub_dir: &Path) -> String {
    format!("{}:{}", stub_dir.display(), std::env::var("PATH").unwrap())
}

pub struct Run {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Run a crate binary with `args` in `cwd` under extra `envs`.
pub fn run_bin(exe: &str, args: &[&str], cwd: &Path, envs: &[(&str, String)]) -> Run {
    let mut command = Command::new(exe);
    command.args(args).current_dir(cwd);
    for (key, value) in envs {
        command.env(key, value);
    }
    let out = command.output().unwrap();
    Run {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// The fake-fork patch file used across tests.
pub const PATCH: &str = "0001-fake-fix.patch";

pub const PATCH_TEXT: &str = "\
From 0000000000000000000000000000000000000000 Mon Sep 17 00:00:00 2001
From: Test <t@t>
Date: Mon, 1 Jan 2026 00:00:00 +0000
Subject: [PATCH] fakefix: repair the frobnicator widget alignment

---
";

pub const DAG_JSON: &str =
    r#"{"comment":"t","base":"deadbeef","nodes":[{"patch":"0001-fake-fix.patch","deps":[]}]}"#;

/// Lay out `<root>/<patch_dir>` with the fake patch + dag.json.
pub fn write_series(root: &Path, patch_dir: &str) -> PathBuf {
    let dir = root.join(patch_dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(PATCH), PATCH_TEXT).unwrap();
    fs::write(dir.join("dag.json"), DAG_JSON).unwrap();
    dir
}

/// Read a fork's status file as JSON.
pub fn status_json(root: &Path, patch_dir: &str) -> serde_json::Value {
    let raw = fs::read_to_string(root.join(patch_dir).join("upstream-status.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}
