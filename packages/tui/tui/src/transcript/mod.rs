//! Tail an agent CLI's own session log into normalized transcript entries.
//!
//! The structured transcript comes from the file the agent itself writes --
//! Claude Code's `~/.claude/projects/<munged-cwd>/<session>.jsonl`, Codex's
//! `~/.codex/sessions/<Y>/<M>/<D>/rollout-*.jsonl` -- not from screen
//! scraping, which is lossy and unparseable. Resolution never walks the whole
//! tree: the producer knows the agent's cwd and start time, so it reads the
//! ONE directory those imply and picks the newest log born after the spawn.
//! (test-ide's server/agent-transcripts.ts documents the cold-walk mistake
//! this avoids: ~95k transcripts on a lived-in machine, ~30s per walk.)
//!
//! *Born*, not merely modified. A cwd routinely already holds a session that
//! is still running, and a running session is the newest thing in that
//! directory by mtime for as long as it lives, so an mtime rule hands it to
//! whichever agent spawns next -- which is what ENG-12529 was. Until the CLI
//! creates the spawned agent's own file there is simply no transcript source
//! yet; that is a normal early state, re-checked each tick, and never a
//! reason to settle for a file that predates the spawn.
//!
//! A log is append-only, so the tail keeps a byte offset and reads only what
//! grew, and only up to the last newline: an append caught mid-write stays
//! pending until its line completes, so a torn entry can never be half-read.
//! A line that parses to no known shape is counted in [`Tail::skipped`],
//! never silently dropped -- a new CLI version changing its format shows up
//! as a climbing number instead of a quietly frozen transcript.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::types::SessionLogKind;

/// Per-entry text cap. The transcript pane is a chat view, not an archive: a
/// pasted wall of text must not balloon the pane body (which is one Loro text
/// container) while a normal reply should show whole.
const TEXT_CAP: usize = 600;

/// How many entries the pane retains. Within the window rows only append, so
/// Loro text diffs stay incremental; when it slides, the head rewrite costs
/// one larger diff. 200 entries outlasts any turn a human is watching live.
const KEEP_ENTRIES: usize = 200;

/// Slack subtracted from the spawn instant when deciding whether a candidate
/// log was born for this agent.
///
/// It covers clock granularity only. It is deliberately small: the whole
/// point of the birth check is that a session already running in this cwd
/// must never be adopted, and every second of slack is a second of window in
/// which one could be.
const START_SLACK: Duration = Duration::from_secs(2);

/// How many leading lines of a candidate log are read looking for the
/// session's own first timestamp. Claude Code opens a session file with a few
/// untimestamped bookkeeping lines (`last-prompt`, `mode`,
/// `permission-mode`) before the first conversation line, so reading only
/// line one would miss it.
const BIRTH_SCAN_LINES: usize = 16;

/// One normalized transcript entry: who said it, what they said, the tool
/// they called, and what it cost. Exactly one of `text`/`tool` is normally
/// non-empty; `usage` rides on Claude's assistant messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TranscriptEntry {
    /// `"user"` or `"assistant"`.
    pub role: String,
    /// The message text, capped at [`TEXT_CAP`] characters.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Tool name for a tool-call entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Token usage, where the log carries it per message (Claude does; Codex
    /// reports usage as separate `token_count` events which would rewrite
    /// already-published rows, so it is left off there).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// The log's own timestamp for the line, when present (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts: Option<String>,
}

/// Token usage for one assistant message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// A live tail over one agent's session log.
///
/// Construction never touches the filesystem; the log usually does not exist
/// yet at spawn (the CLI creates it moments later), so [`poll`](Self::poll)
/// re-resolves until the file appears and tails it from then on.
pub struct Tail {
    kind: SessionLogKind,
    /// The directory the session is keyed by.
    cwd: PathBuf,
    /// Only a log *born* at/after this (minus [`START_SLACK`]) is this
    /// agent's; anything older belongs to a session that already existed.
    started: SystemTime,
    /// Root of the agent's config tree (`~/.claude`, `~/.codex`), resolved
    /// once at construction.
    root: PathBuf,
    file: Option<PathBuf>,
    /// Byte offset just past the last complete line already parsed.
    offset: u64,
    /// The rolling window of normalized entries.
    pub entries: Vec<TranscriptEntry>,
    /// Lines that parsed to no known shape. Monotonic; a growing number is
    /// the signal that the CLI's log format moved under this parser.
    pub skipped: u64,
}

impl Tail {
    /// A tail for one spawned agent. `cwd` defaults to the process cwd and
    /// `root` to the conventional home location when the config leaves them
    /// unset.
    #[must_use]
    pub fn new(
        kind: SessionLogKind,
        cwd: Option<PathBuf>,
        root: Option<PathBuf>,
        started: SystemTime,
    ) -> Self {
        let cwd = cwd.unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
        });
        let root = root.unwrap_or_else(|| default_root(kind));
        Self {
            kind,
            cwd,
            started,
            root,
            file: None,
            offset: 0,
            entries: Vec::new(),
            skipped: 0,
        }
    }

    /// The resolved log path, once one exists.
    #[must_use]
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    /// Fold any newly appended complete lines into `entries`/`skipped`.
    /// Reports whether anything changed.
    pub fn poll(&mut self) -> bool {
        if self.file.is_none() {
            self.file = resolve(self.kind, &self.root, &self.cwd, self.started);
        }
        let Some(file) = self.file.clone() else {
            return false;
        };
        let Ok(meta) = std::fs::metadata(&file) else {
            return false;
        };
        if meta.len() < self.offset {
            // The file shrank: it was recreated. Start over.
            self.offset = 0;
        }
        if meta.len() == self.offset {
            return false;
        }
        let Ok(grown) = read_span(&file, self.offset, meta.len()) else {
            return false;
        };
        // Only complete lines: a torn tail stays pending until its newline.
        let Some(cut) = grown.iter().rposition(|byte| *byte == b'\n') else {
            return false;
        };
        let complete = &grown[..cut];
        self.offset += cut as u64 + 1;
        let mut changed = false;
        for line in complete.split(|byte| *byte == b'\n') {
            let line = String::from_utf8_lossy(line);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let outcome = match self.kind {
                SessionLogKind::Claude => parse_claude_line(line),
                SessionLogKind::Codex => parse_codex_line(line),
            };
            match outcome {
                Parsed::Entries(new) => {
                    changed = changed || !new.is_empty();
                    self.entries.extend(new);
                }
                Parsed::Machinery => {}
                Parsed::Unknown => {
                    self.skipped += 1;
                    changed = true;
                }
            }
        }
        if self.entries.len() > KEEP_ENTRIES {
            let excess = self.entries.len() - KEEP_ENTRIES;
            self.entries.drain(..excess);
        }
        changed
    }
}

/// What one log line was.
enum Parsed {
    /// Conversation: normalized entries (possibly none, e.g. an empty text).
    Entries(Vec<TranscriptEntry>),
    /// A shape this parser knows and deliberately does not surface (titles,
    /// hooks, mode changes, reasoning ciphertext, token counts).
    Machinery,
    /// Not a shape this parser knows: counted, never silent.
    Unknown,
}

/// The conventional root when the spawner passed none.
fn default_root(kind: SessionLogKind) -> PathBuf {
    let env_key = match kind {
        SessionLogKind::Claude => "CLAUDE_CONFIG_DIR",
        SessionLogKind::Codex => "CODEX_HOME",
    };
    if let Some(dir) = std::env::var_os(env_key) {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
    match kind {
        SessionLogKind::Claude => home.join(".claude"),
        SessionLogKind::Codex => home.join(".codex"),
    }
}

/// Find the one session log for an agent spawned at `started` in `cwd`,
/// reading only the single directory that cwd and date imply.
///
/// Returning `None` is the normal early state, not a failure: the CLI has not
/// created its file yet and [`Tail::poll`] asks again next tick. It is never
/// a reason to settle for an older file -- a cwd routinely holds a session
/// that is still running, and adopting it shows one agent another's
/// conversation (ENG-12529).
fn resolve(
    kind: SessionLogKind,
    root: &Path,
    cwd: &Path,
    started: SystemTime,
) -> Option<PathBuf> {
    let floor = started.checked_sub(START_SLACK).unwrap_or(started);
    match kind {
        SessionLogKind::Claude => {
            let dir = root.join("projects").join(munge_cwd(cwd));
            newest_born_since(&dir, floor, |name| {
                Path::new(name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
            })
        }
        SessionLogKind::Codex => resolve_codex(root, cwd, floor),
    }
}

/// Claude Code keys each project directory by the session cwd with every
/// non-alphanumeric byte replaced by `-` (so `/tmp/x.y` becomes `-tmp-x-y`).
fn munge_cwd(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The newest matching file in one directory *born* at/after `floor`.
///
/// Birth, not modification: a session already running in this cwd is written
/// to constantly, so it is always the newest by mtime and an mtime filter
/// hands it over to whichever agent spawns next (ENG-12529). Mtime is still
/// read first, as a free pre-filter -- a file untouched since before `floor`
/// cannot have been created after it -- so [`born_at`] only opens the handful
/// of files that are actually live.
fn newest_born_since(
    dir: &Path,
    floor: SystemTime,
    matches: impl Fn(&str) -> bool,
) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !matches(name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        if meta.modified().is_ok_and(|mtime| mtime < floor) {
            continue;
        }
        let path = entry.path();
        let Some(born) = born_at(&path, &meta) else {
            continue;
        };
        if born < floor {
            continue;
        }
        if best.as_ref().is_none_or(|(at, _)| born > *at) {
            best = Some((born, path));
        }
    }
    best.map(|(_, path)| path)
}

/// When a candidate session log came into being.
///
/// The log's own earliest timestamp is preferred: the CLI wrote it, it
/// survives a copy or a restore, and it is there on every filesystem. The
/// filesystem birth time is the fallback for a file too young to have
/// produced a timestamped line yet (Claude Code opens with a few
/// untimestamped bookkeeping lines).
///
/// `None` means neither is known *yet* -- a just-created, still-empty file on
/// a filesystem without birth times. The caller skips the candidate this
/// tick and asks again on the next one, which is the safe direction: the cost
/// is a transcript that starts a second late, where guessing costs the wrong
/// session entirely.
fn born_at(path: &Path, meta: &std::fs::Metadata) -> Option<SystemTime> {
    first_timestamp(path).or_else(|| filesystem_birth(meta))
}

/// The filesystem's birth time, where it has one.
///
/// A filesystem that does not record birth times reports either an error or a
/// zeroed time; both mean "unknown", and neither may be read as 1970, which
/// would make every candidate look ancient.
fn filesystem_birth(meta: &std::fs::Metadata) -> Option<SystemTime> {
    let born = meta.created().ok()?;
    (born != SystemTime::UNIX_EPOCH).then_some(born)
}

/// The first `timestamp` in the opening [`BIRTH_SCAN_LINES`] lines of a
/// session log, which is when that session started.
fn first_timestamp(path: &Path) -> Option<SystemTime> {
    use std::io::{BufRead as _, BufReader};
    let handle = std::fs::File::open(path).ok()?;
    BufReader::new(handle)
        .lines()
        .take(BIRTH_SCAN_LINES)
        .map_while(std::result::Result::ok)
        .find_map(|line| {
            let value: serde_json::Value = serde_json::from_str(&line).ok()?;
            parse_iso_utc(value.get("timestamp")?.as_str()?)
        })
}

/// Parse the one timestamp shape both CLIs write:
/// `YYYY-MM-DDTHH:MM:SS[.fff]Z`.
///
/// Deliberately strict. Anything else -- a local-time offset, a truncated
/// line, a future format -- is `None`, which falls through to the filesystem
/// birth time rather than to a misread instant.
fn parse_iso_utc(text: &str) -> Option<SystemTime> {
    let (date, time) = text.split_once('T')?;
    let time = time.strip_suffix('Z')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }
    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    // Sub-second precision is irrelevant to a spawn-vs-session comparison.
    let second: i64 = time_parts.next()?.split('.').next()?.parse().ok()?;
    if time_parts.next().is_some() {
        return None;
    }
    let secs = days_from_civil(year, month, day)
        .checked_mul(86_400)?
        .checked_add(hour * 3_600 + minute * 60 + second)?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(u64::try_from(secs).ok()?))
}

/// Calendar date to days since the epoch: Howard Hinnant's days-from-civil,
/// the inverse of [`civil_date`].
const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let yoe = shifted - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Codex writes `sessions/<YYYY>/<MM>/<DD>/rollout-<stamp>-<uuid>.jsonl`. The
/// date directory comes from the filename's own stamp format; rather than
/// reimplement its timezone, scan the day directories whose *dates* straddle
/// `floor` (today and yesterday relative to it), newest match first,
/// preferring a candidate whose `session_meta` first line names `cwd`.
fn resolve_codex(root: &Path, cwd: &Path, floor: SystemTime) -> Option<PathBuf> {
    let mut candidates: Vec<(SystemTime, PathBuf)> = Vec::new();
    for day in day_dirs_around(&root.join("sessions"), floor) {
        if let Ok(entries) = std::fs::read_dir(&day) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                let is_rollout = name.starts_with("rollout-")
                    && Path::new(name)
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"));
                if !is_rollout {
                    continue;
                }
                let Ok(meta) = entry.metadata() else { continue };
                if meta.modified().is_ok_and(|mtime| mtime < floor) {
                    continue;
                }
                let path = entry.path();
                // Born, not modified, for the same reason Claude's resolver
                // uses it: a rollout still being appended to belongs to the
                // session that opened it.
                let Some(born) = born_at(&path, &meta) else {
                    continue;
                };
                if born >= floor {
                    candidates.push((born, path));
                }
            }
        }
    }
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
    let wanted = cwd.to_string_lossy();
    for (_, path) in &candidates {
        if session_meta_cwd(path).is_some_and(|meta_cwd| meta_cwd == wanted) {
            return Some(path.clone());
        }
    }
    candidates.into_iter().next().map(|(_, path)| path)
}

/// The `<sessions>/<Y>/<M>/<D>` directories for the floor's date and the two
/// adjacent dates, existing ones only. Three days of slack covers a spawn
/// near midnight in any timezone without scanning the whole tree.
fn day_dirs_around(sessions: &Path, floor: SystemTime) -> Vec<PathBuf> {
    let day = Duration::from_hours(24);
    [floor.checked_sub(day), Some(floor), floor.checked_add(day)]
        .into_iter()
        .flatten()
        .filter_map(|at| {
            let secs = at.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
            let date = civil_date(secs);
            let dir = sessions
                .join(format!("{:04}", date.year))
                .join(format!("{:02}", date.month))
                .join(format!("{:02}", date.day));
            dir.is_dir().then_some(dir)
        })
        .collect()
}

/// A calendar date.
struct CivilDate {
    year: i64,
    month: i64,
    day: i64,
}

/// Days-since-epoch to a calendar date, Howard Hinnant's civil-from-days.
/// Local-time truncation error is absorbed by scanning adjacent days.
fn civil_date(unix_secs: u64) -> CivilDate {
    #[expect(
        clippy::fallible_int_fallback,
        reason = "u64 seconds / 86400 fits i64 until year ~292 billion; the \
                  fallback is unreachable and a wrong date here only widens \
                  the (already slack) day-directory scan"
    )]
    let days = i64::try_from(unix_secs / 86_400).unwrap_or(0);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    CivilDate {
        year: if month <= 2 { year + 1 } else { year },
        month,
        day,
    }
}

/// Read bytes `[start, end)` of a file.
fn read_span(file: &Path, start: u64, end: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut handle = std::fs::File::open(file)?;
    handle.seek(SeekFrom::Start(start))?;
    let len = usize::try_from(end.saturating_sub(start))
        .map_err(|_| std::io::Error::other("appended span larger than address space"))?;
    let mut buf = vec![0u8; len];
    let mut at = 0;
    while at < buf.len() {
        let n = handle.read(&mut buf[at..])?;
        if n == 0 {
            break;
        }
        at += n;
    }
    buf.truncate(at);
    Ok(buf)
}

/// Cap `text` at [`TEXT_CAP`] characters, marking the cut.
fn capped(text: &str) -> String {
    if text.chars().count() > TEXT_CAP {
        let head: String = text.chars().take(TEXT_CAP).collect();
        format!("{head} …")
    } else {
        text.to_owned()
    }
}

/// One Claude Code JSONL line -> normalized entries.
///
/// Grounded against live transcripts (and test-ide's simplifyLines): only
/// `type` user/assistant lines are conversation. Text parts become one entry
/// per contiguous run; each `tool_use` becomes its own entry named after the
/// tool; `tool_result` parts are plumbing and emit nothing. Every other line
/// carrying a string `type` (titles, hooks, modes, queue ops...) is Claude
/// machinery; a line that is not even that shape is Unknown and counted.
fn parse_claude_line(line: &str) -> Parsed {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return Parsed::Unknown;
    };
    let Some(kind) = value.get("type").and_then(|t| t.as_str()) else {
        return Parsed::Unknown;
    };
    let role = match kind {
        "user" => "user",
        "assistant" => "assistant",
        _ => return Parsed::Machinery,
    };
    let ts = value
        .get("timestamp")
        .and_then(|t| t.as_str())
        .map(str::to_owned);
    let usage = (role == "assistant")
        .then(|| {
            let usage = value.get("message")?.get("usage")?;
            Some(Usage {
                input_tokens: usage.get("input_tokens")?.as_u64()?,
                output_tokens: usage.get("output_tokens")?.as_u64()?,
            })
        })
        .flatten();
    let Some(content) = value.get("message").and_then(|m| m.get("content")) else {
        return Parsed::Unknown;
    };

    let mut out = Vec::new();
    let mut parts: Vec<&str> = Vec::new();
    let flush = |parts: &mut Vec<&str>, out: &mut Vec<TranscriptEntry>| {
        let text = parts.join("\n");
        let text = text.trim();
        parts.clear();
        if text.is_empty() {
            return;
        }
        out.push(TranscriptEntry {
            role: role.to_owned(),
            text: capped(text),
            tool: None,
            usage,
            ts: ts.clone(),
        });
    };
    match content {
        serde_json::Value::String(text) => parts.push(text),
        serde_json::Value::Array(list) => {
            for part in list {
                match part.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            parts.push(text);
                        }
                    }
                    Some("tool_use") => {
                        flush(&mut parts, &mut out);
                        if let Some(name) = part.get("name").and_then(|n| n.as_str()) {
                            out.push(TranscriptEntry {
                                role: role.to_owned(),
                                text: String::new(),
                                tool: Some(name.to_owned()),
                                usage: None,
                                ts: ts.clone(),
                            });
                        }
                    }
                    // tool_result / thinking / images: plumbing for this view.
                    _ => {}
                }
            }
        }
        _ => return Parsed::Unknown,
    }
    flush(&mut parts, &mut out);
    Parsed::Entries(out)
}

/// One Codex rollout JSONL line -> normalized entries.
///
/// Grounded against live rollouts (codex-tui 0.136): conversation rides in
/// `response_item` payloads -- `message` with role user/assistant and
/// `input_text`/`output_text` parts, and `function_call` for tools. The
/// `developer` role is injected instructions, `event_msg`/`turn_context`/
/// `session_meta`/`reasoning` are machinery. A line without the envelope
/// `type` is Unknown and counted.
fn parse_codex_line(line: &str) -> Parsed {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return Parsed::Unknown;
    };
    let Some(kind) = value.get("type").and_then(|t| t.as_str()) else {
        return Parsed::Unknown;
    };
    if kind != "response_item" {
        return Parsed::Machinery;
    }
    let Some(payload) = value.get("payload") else {
        return Parsed::Unknown;
    };
    let ts = value
        .get("timestamp")
        .and_then(|t| t.as_str())
        .map(str::to_owned);
    match payload.get("type").and_then(|t| t.as_str()) {
        Some("message") => {
            let role = match payload.get("role").and_then(|r| r.as_str()) {
                Some("user") => "user",
                Some("assistant") => "assistant",
                // Injected instructions, not conversation.
                Some(_) => return Parsed::Machinery,
                None => return Parsed::Unknown,
            };
            let mut texts: Vec<&str> = Vec::new();
            if let Some(parts) = payload.get("content").and_then(|c| c.as_array()) {
                for part in parts {
                    if matches!(
                        part.get("type").and_then(|t| t.as_str()),
                        Some("input_text" | "output_text")
                    ) && let Some(text) = part.get("text").and_then(|t| t.as_str())
                    {
                        texts.push(text);
                    }
                }
            }
            let text = texts.join("\n");
            let text = text.trim();
            if text.is_empty() {
                return Parsed::Entries(Vec::new());
            }
            Parsed::Entries(vec![TranscriptEntry {
                role: role.to_owned(),
                text: capped(text),
                tool: None,
                usage: None,
                ts,
            }])
        }
        Some("function_call") => {
            let Some(name) = payload.get("name").and_then(|n| n.as_str()) else {
                return Parsed::Unknown;
            };
            Parsed::Entries(vec![TranscriptEntry {
                role: "assistant".to_owned(),
                text: String::new(),
                tool: Some(name.to_owned()),
                usage: None,
                ts,
            }])
        }
        // reasoning ciphertext, function_call_output, web searches: plumbing.
        Some(_) => Parsed::Machinery,
        None => Parsed::Unknown,
    }
}

/// The `cwd` recorded in a Codex rollout's `session_meta` first line, if that
/// is what the first line is.
fn session_meta_cwd(file: &Path) -> Option<String> {
    use std::io::{BufRead as _, BufReader};
    let handle = std::fs::File::open(file).ok()?;
    let mut first = String::new();
    BufReader::new(handle).read_line(&mut first).ok()?;
    let value: serde_json::Value = serde_json::from_str(&first).ok()?;
    (value.get("type")? == "session_meta")
        .then(|| value.get("payload")?.get("cwd")?.as_str().map(str::to_owned))
        .flatten()
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    /// A real-shaped Claude Code line set: text and `tool_use` become entries
    /// in order, usage rides the assistant text, machinery types are ignored
    /// unounted, and garbage is counted.
    #[test]
    fn claude_lines_normalize_and_count_unknowns() {
        let text_line = r#"{"type":"assistant","timestamp":"2026-07-29T11:54:09.709Z","message":{"role":"assistant","usage":{"input_tokens":2,"output_tokens":258},"content":[{"type":"text","text":"I'll dig into this."},{"type":"tool_use","name":"Bash","input":{}}]}}"#;
        let Parsed::Entries(entries) = parse_claude_line(text_line) else {
            panic!("an assistant line is conversation");
        };
        assert_eq!(
            entries,
            vec![
                TranscriptEntry {
                    role: "assistant".to_owned(),
                    text: "I'll dig into this.".to_owned(),
                    tool: None,
                    usage: Some(Usage {
                        input_tokens: 2,
                        output_tokens: 258
                    }),
                    ts: Some("2026-07-29T11:54:09.709Z".to_owned()),
                },
                TranscriptEntry {
                    role: "assistant".to_owned(),
                    text: String::new(),
                    tool: Some("Bash".to_owned()),
                    usage: None,
                    ts: Some("2026-07-29T11:54:09.709Z".to_owned()),
                },
            ]
        );

        let user = r#"{"type":"user","message":{"role":"user","content":"hello there"}}"#;
        let Parsed::Entries(entries) = parse_claude_line(user) else {
            panic!("a user line is conversation");
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, "user");
        assert_eq!(entries[0].text, "hello there");
        assert_eq!(entries[0].usage, None, "usage is an assistant thing");

        // Claude machinery: known shape, no entry, not counted as unknown.
        assert!(matches!(
            parse_claude_line(r#"{"type":"ai-title","aiTitle":"t"}"#),
            Parsed::Machinery
        ));
        // Garbage and typeless lines are the counted kind.
        assert!(matches!(parse_claude_line("not json at all"), Parsed::Unknown));
        assert!(matches!(parse_claude_line(r#"{"no":"type"}"#), Parsed::Unknown));
    }

    /// A real-shaped Codex rollout line set, same contract.
    #[test]
    fn codex_lines_normalize_and_count_unknowns() {
        let reply = r#"{"timestamp":"2026-07-02T17:35:34.929Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Using skills."}]}}"#;
        let Parsed::Entries(entries) = parse_codex_line(reply) else {
            panic!("an assistant response_item is conversation");
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, "assistant");
        assert_eq!(entries[0].text, "Using skills.");

        let call = r#"{"type":"response_item","payload":{"type":"function_call","name":"session_set_name","arguments":"{}","call_id":"c1"}}"#;
        let Parsed::Entries(entries) = parse_codex_line(call) else {
            panic!("a function_call is a tool entry");
        };
        assert_eq!(entries[0].tool.as_deref(), Some("session_set_name"));

        // The developer role is injected instructions, not conversation.
        let developer = r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<permissions>"}]}}"#;
        assert!(matches!(parse_codex_line(developer), Parsed::Machinery));
        // Envelope machinery and unknowns.
        assert!(matches!(
            parse_codex_line(r#"{"type":"event_msg","payload":{"type":"token_count"}}"#),
            Parsed::Machinery
        ));
        assert!(matches!(parse_codex_line("{broken"), Parsed::Unknown));
    }

    /// The tail consumes only complete lines: a torn append stays pending
    /// until its newline lands, then parses whole. Unknown lines climb the
    /// skip count instead of vanishing.
    #[test]
    fn tail_waits_for_the_newline_and_counts_skips() {
        let dir = std::env::temp_dir().join(format!("ix-tui-tail-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("session.jsonl");
        std::fs::write(&file, b"").expect("create");

        let mut tail = Tail::new(
            SessionLogKind::Claude,
            Some(dir.clone()),
            Some(dir.clone()),
            SystemTime::now(),
        );
        // Point the tail at the file directly: resolution is covered by its
        // own test, this one is about the byte discipline.
        tail.file = Some(file.clone());

        let mut handle = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .expect("open");
        let line = r#"{"type":"user","message":{"role":"user","content":"first"}}"#;
        // A torn write: everything but the newline.
        handle.write_all(line.as_bytes()).expect("write");
        handle.flush().expect("flush");
        assert!(!tail.poll(), "a torn line must stay pending");
        assert!(tail.entries.is_empty());

        handle.write_all(b"\n").expect("newline");
        handle.flush().expect("flush");
        assert!(tail.poll(), "the completed line lands");
        assert_eq!(tail.entries.len(), 1);
        assert_eq!(tail.entries[0].text, "first");

        handle.write_all(b"garbage that is not json\n").expect("write");
        handle.flush().expect("flush");
        assert!(tail.poll(), "a counted skip is a visible change");
        assert_eq!(tail.entries.len(), 1, "garbage adds no entry");
        assert_eq!(tail.skipped, 1, "and is counted, never silent");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Claude resolution reads exactly one directory -- the munged-cwd
    /// project dir -- and picks the newest log born after the spawn,
    /// skipping an older session in the same project.
    #[test]
    fn claude_resolution_reads_one_project_dir() {
        let root = std::env::temp_dir().join(format!("ix-tui-claude-{}", std::process::id()));
        let cwd = PathBuf::from("/tmp/agent.work");
        let project = root.join("projects").join("-tmp-agent-work");
        std::fs::create_dir_all(&project).expect("mkdir");

        let old = project.join("old-session.jsonl");
        std::fs::write(&old, b"{}\n").expect("old");
        let hour = Duration::from_hours(1);
        let old_mtime = SystemTime::now().checked_sub(hour).expect("clock");
        set_mtime(&old, old_mtime);

        let started = SystemTime::now();
        let fresh = project.join("fresh-session.jsonl");
        std::fs::write(&fresh, b"{}\n").expect("fresh");

        assert_eq!(
            resolve(SessionLogKind::Claude, &root, &cwd, started),
            Some(fresh),
            "the newest log born after the spawn is the session's"
        );

        let elsewhere = PathBuf::from("/somewhere/else");
        assert_eq!(
            resolve(SessionLogKind::Claude, &root, &elsewhere, started),
            None,
            "another cwd's project dir is not consulted"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A session already running in this cwd is never adopted, however
    /// recently it was written to.
    ///
    /// The ENG-12529 regression: resolution used to rank candidates by mtime,
    /// and a live neighbour is by construction the newest by mtime, so a
    /// freshly spawned agent's pane showed an unrelated session's rows. The
    /// agent's own log does not exist yet at that moment, which is why "no
    /// log yet" has to be an acceptable answer.
    #[test]
    fn a_session_older_than_the_spawn_is_never_adopted() {
        let root = std::env::temp_dir().join(format!(
            "ix-tui-neighbour-{}",
            std::process::id()
        ));
        let cwd = PathBuf::from("/tmp/agent.work");
        let project = root.join("projects").join("-tmp-agent-work");
        std::fs::create_dir_all(&project).expect("mkdir");

        // A neighbour whose session started long before this spawn and which
        // is still being appended to, so it holds the newest mtime here.
        let neighbour = project.join("neighbour.jsonl");
        std::fs::write(
            &neighbour,
            br#"{"type":"user","timestamp":"2020-01-02T03:04:05.000Z","message":{"role":"user","content":"theirs"}}
"#,
        )
        .expect("neighbour");
        let started = SystemTime::now();
        set_mtime(
            &neighbour,
            started.checked_add(Duration::from_hours(1)).expect("clock"),
        );

        assert_eq!(
            resolve(SessionLogKind::Claude, &root, &cwd, started),
            None,
            "another session's log is never this agent's, however fresh its mtime"
        );

        // Once the agent's own log appears it wins, neighbour and all.
        let ours = project.join("ours.jsonl");
        std::fs::write(&ours, b"{\"type\":\"summary\"}\n").expect("ours");
        assert_eq!(
            resolve(SessionLogKind::Claude, &root, &cwd, started),
            Some(ours),
            "the log born at the spawn is the one to tail"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// With no timestamped line yet, the filesystem birth time decides -- and
    /// decides the same way: a file that existed before the spawn is refused.
    #[test]
    fn an_untimestamped_log_falls_back_to_the_birth_time() {
        let root = std::env::temp_dir().join(format!("ix-tui-birth-{}", std::process::id()));
        let cwd = PathBuf::from("/tmp/agent.work");
        let project = root.join("projects").join("-tmp-agent-work");
        std::fs::create_dir_all(&project).expect("mkdir");

        // Claude Code's opening lines carry no timestamp, so this candidate
        // has only its birth time to go on. The forward mtime keeps it past
        // the cheap pre-filter, so it is the birth check that decides.
        let file = project.join("bookkeeping-only.jsonl");
        std::fs::write(&file, b"{\"type\":\"mode\",\"mode\":\"normal\"}\n").expect("write");
        let now = SystemTime::now();
        set_mtime(&file, now.checked_add(Duration::from_hours(1)).expect("clock"));

        assert_eq!(
            resolve(SessionLogKind::Claude, &root, &cwd, now),
            Some(file),
            "a log created at the spawn is this agent's"
        );

        let later_spawn = now.checked_add(START_SLACK * 2).expect("clock");
        assert_eq!(
            resolve(SessionLogKind::Claude, &root, &cwd, later_spawn),
            None,
            "the same log predates a later spawn and must not be handed to it"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The timestamp parser inverts [`civil_date`] and refuses anything that
    /// is not a UTC instant, so an unrecognized stamp falls through to the
    /// birth time instead of becoming a wrong one.
    #[test]
    fn iso_timestamps_parse_and_anything_else_is_refused() {
        let at = parse_iso_utc("2026-07-29T11:54:09.709Z").expect("a real Claude timestamp");
        let secs = at
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs();
        let date = civil_date(secs);
        assert_eq!(date.year, 2026);
        assert_eq!(date.month, 7);
        assert_eq!(date.day, 29);
        assert_eq!(secs % 86_400, 11 * 3600 + 54 * 60 + 9);

        assert_eq!(
            parse_iso_utc("2026-07-29T11:54:09+02:00"),
            None,
            "a local offset is not a UTC instant"
        );
        assert_eq!(parse_iso_utc("2026-07-29"), None, "a date is not an instant");
        assert_eq!(parse_iso_utc("yesterday"), None);
    }

    /// Codex resolution scans the day directories around the spawn and
    /// prefers the rollout whose `session_meta` names the agent's cwd.
    #[test]
    fn codex_resolution_prefers_the_matching_session_meta() {
        let root = std::env::temp_dir().join(format!("ix-tui-codex-{}", std::process::id()));
        let started = SystemTime::now();
        let secs = started
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock")
            .as_secs();
        let date = civil_date(secs);
        let day = root
            .join("sessions")
            .join(format!("{:04}", date.year))
            .join(format!("{:02}", date.month))
            .join(format!("{:02}", date.day));
        std::fs::create_dir_all(&day).expect("mkdir");

        let other = day.join("rollout-a-other.jsonl");
        std::fs::write(
            &other,
            b"{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/elsewhere\"}}\n",
        )
        .expect("other");
        let ours = day.join("rollout-b-ours.jsonl");
        std::fs::write(
            &ours,
            b"{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/tmp/agent.work\"}}\n",
        )
        .expect("ours");

        assert_eq!(
            resolve(
                SessionLogKind::Codex,
                &root,
                Path::new("/tmp/agent.work"),
                started
            ),
            Some(ours)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    fn set_mtime(path: &Path, to: SystemTime) {
        let file = std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .expect("open for mtime");
        file.set_modified(to).expect("set mtime");
    }
}
