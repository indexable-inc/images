//! The detached-worker protocol shared by the out-of-band Stop hooks
//! (`friction-report --analyze`, `retro-gate --dispatch`).
//!
//! Both hooks split the same way: the foreground half must never block Stop,
//! so it re-spawns THIS SAME binary detached — new session via `setsid`,
//! stdin `/dev/null`, stdout+stderr appended to the hook's log — with the
//! Stop payload riding an env var, and returns immediately. The detached half
//! recognizes its argv flag, reads the payload back out of the env, and owns
//! the slow work. Everything here is best-effort and silent (fail OPEN): the
//! log file under the hook's state dir is the only output channel.
//!
//! Each hook declares its identity once as a [`Worker`] const; the protocol
//! lives here so the two halves can never drift apart per hook.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::Value;

/// One hook's detached-worker identity: everything that differs between the
/// hooks that share this protocol.
pub struct Worker {
    /// Subcommand of the detached half: `claude-hooks <subcommand> <flag>`.
    pub subcommand: &'static str,
    /// Argv flag that selects the detached half (e.g. `--analyze`).
    pub flag: &'static str,
    /// Env var the JSON payload rides in between the two halves.
    pub payload_env: &'static str,
    /// Log file name inside the hook's state dir.
    pub log_file: &'static str,
    /// The hook's state dir (each hook's own env-overridable location).
    pub state_dir: fn() -> PathBuf,
}

impl Worker {
    /// The worker's name in log lines: the flag without its dashes.
    fn name(&self) -> &'static str {
        self.flag.trim_start_matches('-')
    }

    /// Timestamped line appended to the hook's log; best-effort, never raises.
    /// This is the only output channel: nothing ever touches stdout/stderr.
    pub fn log(&self, msg: &str) {
        let dir = (self.state_dir)();
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(self.log_file))
        else {
            return;
        };
        let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
        let _ = writeln!(f, "{ts} {msg}");
    }

    /// When argv selects the detached-worker half, read the payload back out
    /// of the env and run `work` on it. True means this process was the
    /// worker and the caller must return immediately, payload or not; false
    /// means it is the foreground half.
    pub fn run_worker(&self, work: impl FnOnce(&Value)) -> bool {
        if !self.invoked() {
            return false;
        }
        if let Some(payload) = self.payload_from_env() {
            work(&payload);
        }
        true
    }

    /// True when argv selects the detached-worker half.
    fn invoked(&self) -> bool {
        std::env::args().skip(1).any(|a| a == self.flag)
    }

    /// The detached half's payload, read back from the env: silently `None`
    /// when the var is missing or not UTF-8, logged when unparseable.
    fn payload_from_env(&self) -> Option<Value> {
        let raw = std::env::var_os(self.payload_env)?;
        let raw = raw.to_str()?;
        let Ok(payload) = serde_json::from_str::<Value>(raw) else {
            self.log(&format!(
                "{}: unparseable {}",
                self.name(),
                self.payload_env
            ));
            return None;
        };
        Some(payload)
    }

    /// Re-spawn THIS binary as `<subcommand> <flag>`, detached (new session,
    /// stdin=/dev/null, stdout+stderr appended to the hook's log), so Stop
    /// returns immediately. Best-effort: any failure is silent.
    pub fn detach(&self, payload: &Value) {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let Ok(payload_json) = serde_json::to_string(payload) else {
            return;
        };
        let dir = (self.state_dir)();
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        let Ok(logf) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(self.log_file))
        else {
            return;
        };
        let Ok(logf2) = logf.try_clone() else {
            return;
        };

        let mut cmd = Command::new(exe);
        cmd.args([self.subcommand, self.flag])
            .env(self.payload_env, payload_json)
            .stdin(Stdio::null())
            .stdout(Stdio::from(logf))
            .stderr(Stdio::from(logf2));
        // start_new_session: own session so it outlives the hook's process tree.
        set_new_session(&mut cmd);
        let _ = cmd.spawn();
        // We deliberately do NOT wait: the child owns the slow work.
    }
}

/// `start_new_session=True` equivalent: call `setsid()` in the child between
/// fork and exec, putting it in a brand-new session and process group (pgid ==
/// pid), detached from the controlling terminal. Used by [`Worker::detach`]
/// and by `friction`'s model child, whose timeout `killpg`s the whole tree.
pub fn set_new_session(cmd: &mut Command) {
    // SAFETY: setsid is async-signal-safe and the only thing we do in the
    // child before exec; no allocation, no shared-state mutation.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}
