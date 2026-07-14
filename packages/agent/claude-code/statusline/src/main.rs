//! House Claude Code statusline (settings `statusLine.command`, shipped in the
//! wrapper's computed settings render, see `passthru.settings`). Renders one
//! dark-gray line: the ix identity mark, context-window bar, model, effort
//! level, and the running CLI version with an "↑<latest>" marker when
//! Anthropic has published a newer release than the wrapper pins.
//!
//! Claude Code pipes a JSON status payload on stdin and re-runs this on every
//! render, so everything here must be fast and fail-soft: the one network call
//! (the `latest` version pointer) is cached for hours and swallowed on error.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use serde::Deserialize;

/// Anthropic's release bucket: `<base>/latest` is a plain-text version string,
/// the same pointer `update.nix` tracks when bumping `manifest.json`.
const RELEASE_BASE: &str = "https://storage.googleapis.com/claude-code-dist-86c565f3-f756-42ad-8dfa-d59b1c096819/claude-code-releases";
const CACHE_TTL: Duration = Duration::from_hours(6);
const FETCH_TIMEOUT: Duration = Duration::from_secs(2);
const BAR_WIDTH: usize = 10;

const DARK_GRAY: &str = "\u{1b}[90m";
const YELLOW: &str = "\u{1b}[33m";
const RESET: &str = "\u{1b}[0m";

#[derive(Parser)]
struct Args {
    /// Last-resort effort level for a settings.json that does not answer
    /// (nothing materialized the render, or the user pruned the key); the
    /// writable settings files win whenever they do.
    #[arg(long, default_value = "")]
    default_effort: String,
}

/// The status payload Claude Code pipes on stdin. Every field is optional
/// (tolerating both absent and null) so schema drift degrades one segment,
/// not the whole line.
#[derive(Deserialize)]
struct Payload {
    version: Option<String>,
    model: Option<Model>,
    context_window: Option<ContextWindow>,
}

#[derive(Deserialize)]
struct Model {
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct ContextWindow {
    context_window_size: Option<usize>,
    current_usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(rename = "input_tokens")]
    input: Option<usize>,
    #[serde(rename = "cache_creation_input_tokens")]
    cache_creation: Option<usize>,
    #[serde(rename = "cache_read_input_tokens")]
    cache_read: Option<usize>,
}

/// The 10-cell context-window bar, floor-percent filled. An explicit zero
/// window size renders an empty bar: this is display code, so degrade rather
/// than die on a division by zero.
fn context_bar(context: Option<&ContextWindow>) -> String {
    let size = context
        .and_then(|c| c.context_window_size)
        .unwrap_or(200_000);
    let total = context
        .and_then(|c| c.current_usage.as_ref())
        .map_or(0, |u| {
            u.input.unwrap_or(0) + u.cache_creation.unwrap_or(0) + u.cache_read.unwrap_or(0)
        });
    let pct = (total * 100).checked_div(size).unwrap_or(0);
    let filled = (pct * BAR_WIDTH / 100).min(BAR_WIDTH);
    format!("{}{}", "█".repeat(filled), "░".repeat(BAR_WIDTH - filled))
}

/// Numeric per-segment compare, so a pinned-ahead `next` build (local >
/// latest) is not flagged as outdated the way plain string inequality would.
/// Any non-numeric segment means "not newer".
fn is_newer(latest: &str, current: &str) -> bool {
    fn segments(version: &str) -> Option<Vec<u64>> {
        version.split('.').map(|part| part.parse().ok()).collect()
    }
    let (Some(l), Some(c)) = (segments(latest), segments(current)) else {
        return false;
    };
    for i in 0..l.len().max(c.len()) {
        let a = l.get(i).copied().unwrap_or(0);
        let b = c.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}

/// Effort cascade: settings.local.json > settings.json > baked default.
fn effort_level(claude_dir: &Path, default_effort: &str) -> String {
    let from_settings = |name: &str| -> Option<String> {
        let text = std::fs::read_to_string(claude_dir.join(name)).ok()?;
        let doc: serde_json::Value = serde_json::from_str(&text).ok()?;
        Some(doc.get("effortLevel")?.as_str()?.to_owned())
    };
    from_settings("settings.local.json")
        .or_else(|| from_settings("settings.json"))
        .unwrap_or_else(|| default_effort.to_owned())
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
}

fn fetch_latest() -> Option<String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .ok()?;
    let text = client
        .get(format!("{RELEASE_BASE}/latest"))
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .ok()?;
    Some(text.trim().to_owned())
}

/// Newest published version, from a small mtime-TTL cache so at most one
/// render per TTL window pays the (2s-capped) fetch. None when offline with a
/// cold cache: the caller then renders the plain version, no marker.
fn latest_version(cache_root: &Path) -> Option<String> {
    let cache_dir = cache_root.join("ix-claude-statusline");
    let cache_file = cache_dir.join("latest");

    let cached_fresh = std::fs::metadata(&cache_file)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < CACHE_TTL);
    if cached_fresh {
        return read_trimmed(&cache_file);
    }

    if let Some(fetched) = fetch_latest().filter(|v| !v.is_empty()) {
        // Best-effort cache write: an unwritable cache dir costs a refetch
        // next render, never the line.
        let _ = std::fs::create_dir_all(&cache_dir);
        let _ = std::fs::write(&cache_file, &fetched);
        return Some(fetched);
    }
    // Stale cache beats nothing while the network is away.
    read_trimmed(&cache_file)
}

/// The rendered line, no trailing newline. Pure (`latest` is threaded in, not
/// fetched) so the tests can pin the render byte-for-byte against fixtures.
fn render(payload: &Payload, effort: &str, latest: Option<&str>) -> String {
    let model = payload
        .model
        .as_ref()
        .and_then(|m| m.display_name.as_deref())
        .unwrap_or("?");
    let bar = context_bar(payload.context_window.as_ref());

    let current = payload.version.as_deref().unwrap_or("").trim();
    let version_segment = match latest {
        _ if current.is_empty() => String::new(),
        Some(l) if is_newer(l, current) => {
            format!(" | v{current} {YELLOW}↑{l}{RESET}{DARK_GRAY}")
        }
        _ => format!(" | v{current}"),
    };
    let effort_segment = if effort.is_empty() {
        String::new()
    } else {
        format!(" | {effort}")
    };

    format!(
        "{DARK_GRAY}⟡ 𝒊𝒙 | {bar} | {model}{effort_segment}{version_segment}{RESET}"
    )
}

/// `$XDG_CACHE_HOME`, else `$HOME/.cache`, else no cache (and so no marker).
fn cache_root(home: Option<&Path>) -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".cache")))
}

fn main() -> ExitCode {
    let args = Args::parse();

    let mut input = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("claude-statusline: read stdin: {err}");
        return ExitCode::FAILURE;
    }
    let payload: Payload = match serde_json::from_str(&input) {
        Ok(payload) => payload,
        Err(err) => {
            eprintln!("claude-statusline: parse status payload: {err}");
            return ExitCode::FAILURE;
        }
    };

    let home = std::env::var_os("HOME").map(PathBuf::from);
    let effort = home.as_deref().map_or_else(
        || args.default_effort.clone(),
        |h| effort_level(&h.join(".claude"), &args.default_effort),
    );

    // Fetch the latest pointer only when there is a version to compare it to.
    let latest = if payload.version.as_deref().unwrap_or("").trim().is_empty() {
        None
    } else {
        cache_root(home.as_deref()).and_then(|root| latest_version(&root))
    };

    println!("{}", render(&payload, &effort, latest.as_deref()));
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::{Payload, effort_level, is_newer, render};

    struct Case {
        fixture: &'static str,
        effort: &'static str,
        latest: Option<&'static str>,
        want: &'static str,
    }

    fn parsed(fixture: &str) -> Payload {
        serde_json::from_str(fixture).expect("fixture parses")
    }

    /// Byte-for-byte renders captured from the retired statusline.nu for the
    /// same stdin fixtures (nu 0.107, 2026-07), so the Rust port is a drop-in.
    #[test]
    fn renders_match_nushell_reference() {
        let cases = [
            Case {
                fixture: include_str!("../fixtures/seeded.json"),
                effort: "high",
                latest: Some("1.2.10"),
                want: "\u{1b}[90m⟡ 𝒊𝒙 | █████░░░░░ | TestModel | high | v1.2.3 \u{1b}[33m↑1.2.10\u{1b}[0m\u{1b}[90m\u{1b}[0m",
            },
            Case {
                fixture: include_str!("../fixtures/no_usage.json"),
                effort: "high",
                latest: Some("1.2.10"),
                want: "\u{1b}[90m⟡ 𝒊𝒙 | ░░░░░░░░░░ | M | high | v1.2.3 \u{1b}[33m↑1.2.10\u{1b}[0m\u{1b}[90m\u{1b}[0m",
            },
            Case {
                fixture: include_str!("../fixtures/empty.json"),
                effort: "high",
                latest: None,
                want: "\u{1b}[90m⟡ 𝒊𝒙 | ░░░░░░░░░░ | ? | high\u{1b}[0m",
            },
            Case {
                fixture: include_str!("../fixtures/model_only.json"),
                effort: "",
                latest: None,
                want: "\u{1b}[90m⟡ 𝒊𝒙 | ░░░░░░░░░░ | M\u{1b}[0m",
            },
            Case {
                fixture: include_str!("../fixtures/ahead.json"),
                effort: "low",
                latest: Some("1.2.10"),
                want: "\u{1b}[90m⟡ 𝒊𝒙 | ░░░░░░░░░░ | M | low | v9.9.9\u{1b}[0m",
            },
            Case {
                fixture: include_str!("../fixtures/full_bar.json"),
                effort: "high",
                latest: Some("1.2.10"),
                want: "\u{1b}[90m⟡ 𝒊𝒙 | ██████████ | M | high | v1.2.3 \u{1b}[33m↑1.2.10\u{1b}[0m\u{1b}[90m\u{1b}[0m",
            },
            Case {
                fixture: include_str!("../fixtures/partial.json"),
                effort: "medium",
                latest: None,
                want: "\u{1b}[90m⟡ 𝒊𝒙 | ██░░░░░░░░ | M | medium\u{1b}[0m",
            },
        ];
        for case in cases {
            let got = render(&parsed(case.fixture), case.effort, case.latest);
            assert_eq!(got, case.want, "render diverged for {}", case.fixture);
        }
    }

    #[test]
    fn is_newer_compares_segments_numerically() {
        assert!(is_newer("1.2.10", "1.2.3"));
        assert!(!is_newer("1.2.3", "1.2.10"), "string compare would flip this");
        assert!(!is_newer("1.2.3", "1.2.3"));
        assert!(is_newer("1.2.3.1", "1.2.3"), "longer latest pads current with zeros");
        assert!(!is_newer("1.2.3", "1.2.3.0"));
        assert!(!is_newer("1.2.3-beta", "1.2.3"), "non-numeric segment means not newer");
        assert!(!is_newer("", "1.2.3"));
    }

    #[test]
    fn effort_cascade_prefers_local_then_user_then_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();
        assert_eq!(effort_level(path, "baked"), "baked");

        std::fs::write(path.join("settings.json"), r#"{"effortLevel":"user"}"#)
            .expect("write settings.json");
        assert_eq!(effort_level(path, "baked"), "user");

        std::fs::write(
            path.join("settings.local.json"),
            r#"{"effortLevel":"local"}"#,
        )
        .expect("write settings.local.json");
        assert_eq!(effort_level(path, "baked"), "local");

        std::fs::write(path.join("settings.local.json"), "not json")
            .expect("overwrite settings.local.json");
        assert_eq!(
            effort_level(path, "baked"),
            "user",
            "unreadable layer falls through, not out"
        );
    }
}
