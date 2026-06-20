//! `SessionStart` banner, the compiled port of the personal `session-start-hook`
//! bash script: report the host's live timezone/OS/kernel/hardware and an
//! auto-discovered inventory of local git repos under `~/Projects`, grouped by
//! org. Emitted as `SessionStart` `additionalContext`. Best-effort: any probe
//! that fails is simply omitted, and the hook never errors.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use crate::ContextOutput;

/// Run `prog args...` and return trimmed stdout, or None on failure / empty.
fn run(prog: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(prog).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

/// Same with an extra env var (used for the `TZ=...` date line).
fn run_env(prog: &str, args: &[&str], key: &str, val: &str) -> Option<String> {
    let out = Command::new(prog).args(args).env(key, val).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

/// IANA timezone name from the `/etc/localtime` symlink target, falling back to
/// the current abbreviation.
fn host_timezone() -> Option<String> {
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let s = target.to_string_lossy();
        if let Some(idx) = s.find("/zoneinfo/") {
            let name = &s[idx + "/zoneinfo/".len()..];
            if !name.is_empty() {
                return Some(name.to_owned());
            }
        }
    }
    run("date", &["+%Z"])
}

/// Inventory of repos under `~/Projects`, grouped by org. A directory counts as
/// a repo when it has a `.git` entry (file or dir, so worktrees count), which
/// also stops the scan from descending into a repo's own subdirs.
fn repo_inventory(out: &mut String) {
    let projects = crate::home().join("Projects");
    let Ok(entries) = std::fs::read_dir(&projects) else {
        return;
    };
    let _ = writeln!(out, "repos (~/Projects/<org>/<repo>):");
    let mut orgs: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    orgs.sort();
    let mut loose: Vec<String> = Vec::new();
    for org in &orgs {
        // A top-level repo (org dir is itself a checkout): list it under (top-level).
        if org.join(".git").exists() {
            if let Some(name) = org.file_name() {
                loose.push(name.to_string_lossy().into_owned());
            }
            continue;
        }
        let Ok(children) = std::fs::read_dir(org) else {
            continue;
        };
        let mut names: Vec<String> = children
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.join(".git").exists())
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        names.sort();
        if !names.is_empty() {
            let org_name = org.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let _ = writeln!(out, "  {org_name}: {}", names.join(" "));
        }
    }
    if !loose.is_empty() {
        let _ = writeln!(out, "  (top-level): {}", loose.join(" "));
    }
}

pub fn session_banner() {
    let mut out = String::new();

    if let Some(tz) = host_timezone() {
        let now = run("date", &["+%Z %z"]).unwrap_or_default();
        let _ = writeln!(out, "host timezone: {tz} ({now})");
    }
    if let Some(d) = run_env("date", &[], "TZ", "America/Los_Angeles") {
        let _ = writeln!(out, "date (TZ=America/Los_Angeles date): {d}");
    }

    // OS: prefer macOS `sw_vers`, else `uname -sr`.
    if Path::new("/usr/bin/sw_vers").exists() || run("sw_vers", &["-productName"]).is_some() {
        let name = run("sw_vers", &["-productName"]).unwrap_or_default();
        let ver = run("sw_vers", &["-productVersion"]).unwrap_or_default();
        let build = run("sw_vers", &["-buildVersion"]).unwrap_or_default();
        let _ = writeln!(out, "os (sw_vers): {name} {ver} ({build})");
    } else if let Some(u) = run("uname", &["-sr"]) {
        let _ = writeln!(out, "os (uname -sr): {u}");
    }

    if let Some(k) = run("uname", &["-srm"]) {
        let _ = writeln!(out, "kernel (uname -srm): {k}");
    }

    if let Some(model) = run("sysctl", &["-n", "hw.model"]) {
        let _ = writeln!(out, "hardware model (sysctl -n hw.model): {model}");
    }
    if let Some(chip) = run("sysctl", &["-n", "machdep.cpu.brand_string"]) {
        let _ = writeln!(out, "hardware chip (sysctl -n machdep.cpu.brand_string): {chip}");
    }

    repo_inventory(&mut out);

    let context = out.trim_end().to_owned();
    if context.is_empty() {
        return;
    }
    crate::emit(ContextOutput {
        hook_event_name: "SessionStart",
        additional_context: context,
    });
}
