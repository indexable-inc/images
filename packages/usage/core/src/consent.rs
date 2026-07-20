//! Consent resolution and the first-run notice or prompt.
//!
//! Precedence, first match wins: `IX_USAGE` env, `DO_NOT_TRACK` env, the
//! config file, then default-on (announced by a one-time notice). CI and
//! non-interactive processes never prompt. Only uploads are gated; local
//! recording always stays on so a later opt-in has history.

use std::io::{BufRead as _, ErrorKind, IsTerminal as _, Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// `~/.config/ix/usage.toml`.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
pub struct Config {
    /// Whether aggregate counts may be uploaded. Local recording is not
    /// gated by this; it only controls the network.
    pub enabled: Option<bool>,
    /// Collector endpoint override.
    pub endpoint: Option<String>,
}

/// Which layer decided the consent outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `IX_USAGE` environment variable.
    EnvIxUsage,
    /// `DO_NOT_TRACK` environment variable (always a refusal).
    DoNotTrack,
    /// The config file.
    ConfigFile,
    /// No layer decided; the documented default (on).
    DefaultOn,
}

impl Source {
    /// Stable identifier used in `--json` output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EnvIxUsage => "env:IX_USAGE",
            Self::DoNotTrack => "env:DO_NOT_TRACK",
            Self::ConfigFile => "config",
            Self::DefaultOn => "default",
        }
    }
}

/// A resolved consent decision plus the layer that decided it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Consent {
    /// Whether uploads are allowed.
    pub upload: bool,
    /// The deciding layer.
    pub source: Source,
}

fn truthy(value: &str) -> bool {
    !matches!(value.trim(), "" | "0" | "false" | "no" | "off")
}

/// Pure consent resolution over already-read inputs (testable without
/// touching the process environment).
#[must_use]
pub fn resolve_with(
    ix_usage: Option<&str>,
    do_not_track: Option<&str>,
    config: Option<&Config>,
) -> Consent {
    if let Some(value) = ix_usage {
        return Consent {
            upload: truthy(value),
            source: Source::EnvIxUsage,
        };
    }
    if do_not_track.is_some_and(truthy) {
        return Consent {
            upload: false,
            source: Source::DoNotTrack,
        };
    }
    if let Some(enabled) = config.and_then(|config| config.enabled) {
        return Consent {
            upload: enabled,
            source: Source::ConfigFile,
        };
    }
    Consent {
        upload: true,
        source: Source::DefaultOn,
    }
}

/// Resolve consent from the live environment and config file.
///
/// # Errors
/// Fails when a config file exists but cannot be read or parsed; callers
/// that cannot surface the error should treat that as upload-off, never as
/// consent.
pub fn resolve() -> anyhow::Result<Consent> {
    let ix_usage = std::env::var("IX_USAGE").ok();
    let do_not_track = std::env::var("DO_NOT_TRACK").ok();
    let config = read_config()?;
    Ok(resolve_with(
        ix_usage.as_deref(),
        do_not_track.as_deref(),
        config.as_ref(),
    ))
}

/// `CI` set truthy: never prompt, and uploads are tagged `ci: true`.
#[must_use]
pub fn is_ci() -> bool {
    std::env::var("CI").is_ok_and(|value| truthy(&value))
}

/// Read the config file; `Ok(None)` when it does not exist (or no home).
///
/// # Errors
/// A config file that exists but cannot be read or parsed.
pub fn read_config() -> anyhow::Result<Option<Config>> {
    let Some(path) = crate::paths::config_path() else {
        return Ok(None);
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    Ok(Some(toml::from_str(&text)?))
}

/// Write the config file, creating parent directories.
///
/// # Errors
/// No home directory, or filesystem/serialization failures.
pub fn write_config(config: &Config) -> anyhow::Result<PathBuf> {
    let Some(path) = crate::paths::config_path() else {
        anyhow::bail!("no home directory; cannot write consent config");
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, toml::to_string_pretty(config)?)?;
    Ok(path)
}

/// What [`first_run`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstRun {
    /// A config file already exists (or another process just won the race).
    AlreadyConfigured,
    /// Consent is managed by environment (`IX_USAGE`, `DO_NOT_TRACK`, CI);
    /// no file written, nothing printed.
    EnvManaged,
    /// No home directory; nothing to do (nix sandbox).
    SandboxNoop,
    /// Interactive prompt answered yes (or enter).
    PromptedYes,
    /// Interactive prompt answered no.
    PromptedNo,
    /// Non-interactive: notice printed to stderr, default-on recorded.
    NoticePrinted,
}

/// The interactive first-run prompt, shown once per install.
///
/// The payload sample is literal on purpose: nobody trusts prose about
/// telemetry, so we show the bytes instead.
const PROMPT: &str = "\nWelcome to ix.\n\n\
To make ix better we count how often each tool runs and how often it\n\
fails. Counts only: the contents of errors, your commands, and your\n\
code never leave this machine. Sent at most once a day. Here is\n\
exactly what a report looks like:\n\n\
  { \"install\": \"b3f1...\", \"os\": \"macos\", \"arch\": \"aarch64\",\n\
    \"counts\": [ { \"pkg\": \"clippy\", \"runs\": 41, \"failures\": 3 } ] }\n\n\
Turn off anytime: `ix-usage off`.\n\n\
Share anonymous usage counts? [Yes] / no: ";

const NOTICE: &str = "ix: anonymous usage counts are on (counts only; error contents never leave this machine).\n\
ix: opt out: `ix-usage off` or enabled=false in ~/.config/ix/usage.toml or DO_NOT_TRACK=1.\n";

/// Handle first run: create the config exactly once.
///
/// Prompts when a human terminal is attached and prints a stderr notice
/// otherwise. Call this after the wrapped tool has finished so its output is
/// never interleaved.
///
/// Prompts write to `/dev/tty` and read from `/dev/tty`; notices go to
/// stderr; stdout is never touched, so piped output cannot be corrupted.
/// Parallel first runs race on an `O_EXCL` create and exactly one process
/// prompts.
///
/// # Errors
/// Filesystem failures creating or writing the config file.
pub fn first_run() -> anyhow::Result<FirstRun> {
    if std::env::var_os("IX_USAGE").is_some()
        || std::env::var_os("DO_NOT_TRACK").is_some()
        || is_ci()
    {
        return Ok(FirstRun::EnvManaged);
    }
    let Some(path) = crate::paths::config_path() else {
        return Ok(FirstRun::SandboxNoop);
    };
    if path.exists() {
        return Ok(FirstRun::AlreadyConfigured);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Win the race before prompting: losers see the file and stay silent.
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            return Ok(FirstRun::AlreadyConfigured);
        }
        Err(err) => return Err(err.into()),
    };

    let answer = if std::io::stderr().is_terminal() {
        prompt_yes_no()
    } else {
        None
    };
    let outcome = match answer {
        Some(true) => FirstRun::PromptedYes,
        Some(false) => FirstRun::PromptedNo,
        None => {
            let mut stderr = std::io::stderr().lock();
            let _ = stderr.write_all(NOTICE.as_bytes());
            FirstRun::NoticePrinted
        }
    };
    let enabled = outcome != FirstRun::PromptedNo;
    let config = Config {
        enabled: Some(enabled),
        endpoint: None,
    };
    file.write_all(toml::to_string_pretty(&config)?.as_bytes())?;
    Ok(outcome)
}

/// Prompt on `/dev/tty`; `None` when no controlling terminal is available
/// (callers fall back to the notice).
fn prompt_yes_no() -> Option<bool> {
    let mut tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    tty.write_all(PROMPT.as_bytes()).ok()?;
    tty.flush().ok()?;
    let mut line = String::new();
    let mut reader = std::io::BufReader::new(ByteTty { inner: &mut tty });
    reader.read_line(&mut line).ok()?;
    Some(!matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "no" | "n"
    ))
}

/// Adapter so the reader half borrows the same `/dev/tty` handle.
struct ByteTty<'a> {
    inner: &'a mut std::fs::File,
}

impl Read for ByteTty<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, Consent, Source, resolve_with};

    const fn config(enabled: Option<bool>) -> Config {
        Config {
            enabled,
            endpoint: None,
        }
    }

    #[test]
    fn env_wins_over_everything() {
        let consent = resolve_with(Some("0"), None, Some(&config(Some(true))));
        assert_eq!(
            consent,
            Consent {
                upload: false,
                source: Source::EnvIxUsage
            }
        );
        let consent = resolve_with(Some("1"), Some("1"), Some(&config(Some(false))));
        assert_eq!(
            consent,
            Consent {
                upload: true,
                source: Source::EnvIxUsage
            }
        );
    }

    #[test]
    fn do_not_track_wins_over_config() {
        let consent = resolve_with(None, Some("1"), Some(&config(Some(true))));
        assert_eq!(
            consent,
            Consent {
                upload: false,
                source: Source::DoNotTrack
            }
        );
    }

    #[test]
    fn config_decides_when_env_is_silent() {
        let consent = resolve_with(None, Some("0"), Some(&config(Some(false))));
        assert_eq!(
            consent,
            Consent {
                upload: false,
                source: Source::ConfigFile
            }
        );
    }

    #[test]
    fn default_is_on() {
        let consent = resolve_with(None, None, Some(&config(None)));
        assert_eq!(
            consent,
            Consent {
                upload: true,
                source: Source::DefaultOn
            }
        );
        let consent = resolve_with(None, None, None);
        assert_eq!(
            consent,
            Consent {
                upload: true,
                source: Source::DefaultOn
            }
        );
    }
}
