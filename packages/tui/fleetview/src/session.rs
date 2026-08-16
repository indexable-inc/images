//! One dispatched agent: the PTY-backed process plus the poller that decides,
//! from screen activity alone, whether it is working or waiting for you.
//!
//! There is no "turn finished" event on a PTY, so the state is inferred the
//! same way [`tui.harness`](../../tui-py/python/tui/harness.py) infers it:
//! **quiescence first, marker second**. The viewport must stop changing for
//! [`SETTLE`], and Claude Code's grounded busy footer ([`BUSY_MARKER`]) must be
//! absent. Quiescence is the version-independent backstop; the marker is the
//! precise fast path that catches an agent thinking silently for longer than
//! the settle window.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tui::{ExitState, SpawnConfig, TuiInstance, TuiManager};
use uuid::Uuid;

/// Footer Claude Code paints while a turn is in flight. Grounded in the Python
/// harness, which uses this exact substring as its busy marker.
const BUSY_MARKER: &str = "esc to interrupt";

/// Fragments of a blocking question. A gate is still "awaiting input", but it
/// is the kind that blocks the agent rather than the kind that means the turn
/// ended, so it gets its own glyph in the list.
const GATE_MARKERS: [&str; 4] = ["do you want", "would you like", "❯ 1. yes", "1. yes, and"];

/// How long the screen must hold still before a session counts as idle. Long
/// enough to survive a spinner frame or a token trickling in, short enough that
/// the list reacts within a breath of the agent stopping.
const SETTLE: Duration = Duration::from_millis(900);

/// Gap between viewport reads. Each read is one round trip through the PTY
/// actor, so this is the per-session polling cost.
const POLL_INTERVAL: Duration = Duration::from_millis(120);

/// What a session is doing, in the three buckets the header counts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// The screen is changing, or the busy footer is up.
    Working,
    /// Alive and quiet: the turn ended, or a question is blocking it.
    AwaitingInput,
    /// The process exited.
    Completed,
}

impl Status {
    /// The section heading this status is listed under.
    pub const fn heading(self) -> &'static str {
        match self {
            Self::AwaitingInput => "awaiting input",
            Self::Working => "working",
            Self::Completed => "completed",
        }
    }
}

/// A point-in-time read of one session's activity.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub status: Status,
    /// The agent is blocked on a question, not merely idle.
    pub gate: bool,
    /// Last line of the screen with anything in it, for the list preview.
    pub preview: String,
    /// Exit code, once the process is gone and had one.
    pub exit_code: Option<i32>,
}

/// Shared between the poller thread and the UI thread.
#[derive(Debug)]
struct Activity {
    last_change: Instant,
    digest: u64,
    busy: bool,
    gate: bool,
    preview: String,
    exited: Option<ExitState>,
}

impl Activity {
    fn new() -> Self {
        Self {
            last_change: Instant::now(),
            digest: 0,
            busy: false,
            gate: false,
            preview: String::new(),
            exited: None,
        }
    }

    /// Fold one viewport read into the activity state.
    fn observe(&mut self, lines: &[String]) {
        let digest = digest(lines);
        if digest != self.digest {
            self.digest = digest;
            self.last_change = Instant::now();
        }
        let haystack = lines.join("\n").to_lowercase();
        self.busy = haystack.contains(BUSY_MARKER);
        self.gate = GATE_MARKERS.iter().any(|marker| haystack.contains(marker));
        if let Some(preview) = preview_line(lines) {
            self.preview = preview;
        }
    }

    fn snapshot(&self) -> Snapshot {
        let (status, exit_code) = match self.exited {
            Some(ExitState::Exited(code)) => (Status::Completed, code),
            // A running process whose screen is still moving, or whose footer
            // says it is thinking, is working; anything else wants a human.
            _ if self.busy || self.last_change.elapsed() < SETTLE => (Status::Working, None),
            _ => (Status::AwaitingInput, None),
        };
        Snapshot {
            status,
            gate: self.gate && status == Status::AwaitingInput,
            preview: self.preview.clone(),
            exit_code,
        }
    }
}

/// A stable digest of the rendered screen. Any cell that changed changes this.
fn digest(lines: &[String]) -> u64 {
    let mut hasher = DefaultHasher::new();
    lines.hash(&mut hasher);
    hasher.finish()
}

/// Chrome an agent TUI paints every frame regardless of what it is doing. It is
/// the bottom of the screen, so a naive "last non-empty line" preview shows
/// nothing but this; a preview is only worth a column if it says what changed.
const CHROME: [&str; 7] = [
    "esc to interrupt",
    "shift+tab",
    "bypass permissions",
    "auto-accept edits",
    "plan mode on",
    "for shortcuts",
    "ctrl+",
];

/// What the agent most recently said, for the row's middle column.
///
/// Claude Code bullets every message and tool call with `⏺`, and that line is
/// exactly the one worth showing: the reply when a turn ends, the running tool
/// while it works. Agents that do not bullet fall back to the last line of
/// content above the input chrome.
fn preview_line(lines: &[String]) -> Option<String> {
    bulleted(lines).or_else(|| trailing_content(lines))
}

/// The last bulleted agent message.
fn bulleted(lines: &[String]) -> Option<String> {
    lines.iter().rev().find_map(|line| {
        let inner = undecorate(line);
        let text = inner.strip_prefix(['⏺', '●'])?.trim();
        text.chars()
            .any(char::is_alphanumeric)
            .then(|| text.to_owned())
    })
}

/// The last line of content above the chrome, for an agent with no bullets.
///
/// Everything below the input area is chrome: the agent's own footer, and a
/// statusline the user configures, which can say anything at all and so cannot
/// be recognised by pattern. The rule or box that separates the input from the
/// transcript is the structural landmark every agent TUI has, so cut there.
fn trailing_content(lines: &[String]) -> Option<String> {
    let cut = lines
        .iter()
        .rposition(|line| is_separator(line))
        .unwrap_or(lines.len());
    lines
        .get(..cut)
        .unwrap_or(lines)
        .iter()
        .rev()
        .find_map(|line| content(line))
        // An agent that has painted no separator yet still gets a preview.
        .or_else(|| lines.iter().rev().find_map(|line| content(line)))
}

/// A line that is only box or rule drawing: the edge of the input area.
fn is_separator(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.chars().count() >= 3
        && trimmed
            .chars()
            .all(|c| matches!(c, '─' | '━' | '│' | '╭' | '╮' | '╰' | '╯' | '├' | '┤' | ' '))
}

/// A line with the surrounding box drawing removed.
fn undecorate(line: &str) -> &str {
    line.trim()
        .trim_start_matches(['│', '╭', '╰', '├', '─', ' '])
        .trim_end_matches(['│', '╮', '╯', '┤', '─', ' '])
        .trim()
}

/// One line's content, or `None` if it is decoration, chrome, or the prompt.
fn content(line: &str) -> Option<String> {
    let lowered = line.trim().to_lowercase();
    if CHROME.iter().any(|chrome| lowered.contains(chrome)) {
        return None;
    }
    let inner = undecorate(line);
    // `❯ 1. Yes` is a menu choice worth previewing; a bare `❯`, `>` or `›` is
    // the input prompt, which never is.
    if inner.starts_with(['>', '›']) {
        return None;
    }
    // Claude Code bullets its messages with `⏺`; other agents use `●`/`•`.
    let text = inner
        .trim_start_matches(['❯', '*', '·', '•', '●', '⏺', ' '])
        .trim();
    text.chars()
        .any(char::is_alphanumeric)
        .then(|| text.to_owned())
}

/// Everything needed to dispatch one agent.
#[derive(Clone, Debug)]
pub struct Spec {
    /// The agent binary, e.g. `claude`.
    pub command: String,
    /// Directory the agent runs in.
    pub cwd: PathBuf,
    /// The task typed at the prompt. Empty starts a bare interactive session.
    pub task: String,
    /// `claude --agent NAME`, when the task named a subagent.
    pub agent: Option<String>,
    /// `claude --model NAME`.
    pub model: Option<String>,
    /// Extra arguments appended verbatim to every dispatch.
    pub extra: Vec<String>,
}

/// A dispatched agent and its live activity.
pub struct Session {
    pub id: Uuid,
    pub task: String,
    pub agent: Option<String>,
    pub started: Instant,
    pub instance: TuiInstance,
    activity: Arc<Mutex<Activity>>,
    stop: Arc<AtomicBool>,
}

impl Session {
    /// Spawn `spec` on a PTY sized `rows` x `cols` and start watching it.
    pub fn spawn(manager: &TuiManager, spec: &Spec, rows: u16, cols: u16) -> tui::Result<Self> {
        // Spawn through `sh -c 'cd … && exec …'` rather than chdir of this
        // process: sessions in different repos run at once and a process-global
        // chdir would race them.
        let script = launch_script(spec);
        let config = SpawnConfig {
            rows,
            cols,
            scrollback_lines: 50_000,
            env: vec![("FLEETVIEW".to_owned(), "1".to_owned())],
            // Fleetview keeps its own status inference until it consumes
            // tui::publish (ENG-12457 phase 6); declaring the agent here
            // would matter only to a dashboard producer this binary does not
            // run yet.
            agent: None,
        };
        let instance = manager.spawn("sh".to_owned(), vec!["-c".to_owned(), script], config)?;

        let activity = Arc::new(Mutex::new(Activity::new()));
        let stop = Arc::new(AtomicBool::new(false));
        thread::spawn({
            let instance = instance.clone();
            let activity = Arc::clone(&activity);
            let stop = Arc::clone(&stop);
            move || poll(&instance, &activity, &stop)
        });

        Ok(Self {
            id: instance.id,
            task: spec.task.clone(),
            agent: spec.agent.clone(),
            started: Instant::now(),
            instance,
            activity,
            stop,
        })
    }

    /// The current activity read. Never blocks on the PTY: the poller thread
    /// does that, this only takes the mutex.
    pub fn snapshot(&self) -> Snapshot {
        self.activity.lock().map_or_else(
            |poisoned| poisoned.into_inner().snapshot(),
            |a| a.snapshot(),
        )
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// A short label for the list: the task, or the command when there is none.
    pub fn title(&self) -> &str {
        if self.task.is_empty() {
            "interactive session"
        } else {
            &self.task
        }
    }

    /// The title with the subagent it runs under, in the `+agent task` spelling
    /// the prompt accepts, so a row can be retyped as the command that made it.
    pub fn label(&self) -> String {
        self.agent.as_ref().map_or_else(
            || self.title().to_owned(),
            |agent| format!("+{agent} {}", self.title()),
        )
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Retire the poller with the session; a thread left reading a dead
        // terminal is the "monitor watching a finished writer" bug in miniature.
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Read the viewport until the child exits or the session is dropped.
fn poll(instance: &TuiInstance, activity: &Mutex<Activity>, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        let alive = instance.is_alive();
        if let Ok(lines) = instance.read_viewport()
            && let Ok(mut state) = activity.lock()
        {
            state.observe(&lines);
        }
        if !alive {
            // Record the exit after the final read, so the last screen the agent
            // painted is the one the list previews.
            if let Ok(mut state) = activity.lock() {
                state.exited = Some(instance.exit_state());
            }
            return;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// The `sh -c` script that launches one agent.
fn launch_script(spec: &Spec) -> String {
    let mut words = vec![quote(&spec.command)];
    if let Some(agent) = &spec.agent {
        words.push("--agent".to_owned());
        words.push(quote(agent));
    }
    if let Some(model) = &spec.model {
        words.push("--model".to_owned());
        words.push(quote(model));
    }
    words.extend(spec.extra.iter().map(|arg| quote(arg)));
    if !spec.task.is_empty() {
        // The prompt goes in as an argument rather than typed into the box:
        // typing races the agent's startup paint, an argv does not.
        words.push("--".to_owned());
        words.push(quote(&spec.task));
    }
    format!("cd {} && exec {}", quote_path(&spec.cwd), words.join(" "))
}

/// Single-quote one word for `sh`.
fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

fn quote_path(path: &Path) -> String {
    quote(&path.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::{Activity, Spec, Status, launch_script, preview_line, quote};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};
    use tui::ExitState;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    fn spec(task: &str) -> Spec {
        Spec {
            command: "claude".to_owned(),
            cwd: PathBuf::from("/repo"),
            task: task.to_owned(),
            agent: None,
            model: None,
            extra: Vec::new(),
        }
    }

    #[test]
    fn a_changing_screen_is_working() {
        let mut activity = Activity::new();
        activity.observe(&lines("first paint"));
        assert_eq!(activity.snapshot().status, Status::Working);
    }

    #[test]
    fn a_quiet_screen_awaits_input() {
        let mut activity = Activity::new();
        activity.observe(&lines("done, over to you"));
        // Pretend the last change was long enough ago to settle.
        activity.last_change = Instant::now() - Duration::from_secs(5);
        assert_eq!(activity.snapshot().status, Status::AwaitingInput);
    }

    #[test]
    fn the_busy_footer_beats_quiescence() {
        let mut activity = Activity::new();
        activity.observe(&lines("Thinking…\n  (esc to interrupt)"));
        activity.last_change = Instant::now() - Duration::from_secs(60);
        // A silent agent still holds the footer up, so it is not idle.
        assert_eq!(activity.snapshot().status, Status::Working);
    }

    #[test]
    fn a_question_is_flagged_as_a_gate() {
        let mut activity = Activity::new();
        activity.observe(&lines("Do you want to run this command?\n ❯ 1. Yes"));
        activity.last_change = Instant::now() - Duration::from_secs(5);
        let snapshot = activity.snapshot();
        assert_eq!(snapshot.status, Status::AwaitingInput);
        assert!(snapshot.gate);
    }

    #[test]
    fn a_working_session_is_never_a_gate() {
        let mut activity = Activity::new();
        activity.observe(&lines("Do you want to proceed? (esc to interrupt)"));
        assert!(!activity.snapshot().gate);
    }

    #[test]
    fn an_exited_process_is_completed_whatever_the_screen_says() {
        let mut activity = Activity::new();
        activity.observe(&lines("still painting (esc to interrupt)"));
        activity.exited = Some(ExitState::Exited(Some(2)));
        let snapshot = activity.snapshot();
        assert_eq!(snapshot.status, Status::Completed);
        assert_eq!(snapshot.exit_code, Some(2));
    }

    /// Every screen shape an agent paints, and the one line worth previewing.
    /// The Claude Code cases are copied off live sessions (v2.1.220).
    #[test]
    fn preview_is_the_last_thing_the_agent_said() {
        let cases = [
            (
                "a finished turn: the reply, over the rules and the statusline",
                concat!(
                    "⏺ FLEETVIEW LIVES\n",
                    "✻ Cooked for 2s\n",
                    "────────────────────────────────────────\n",
                    "❯\n",
                    "────────────────────────────────────────\n",
                    "  ⟡ ix | ░░░░░ | Opus 5 (1M context) | high | v2.1.220\n",
                    "  ⏵⏵ bypass permissions on (shift+tab to cycle) · ← 1 agent\n",
                ),
                "FLEETVIEW LIVES",
            ),
            (
                "a boxed input and a footer below it",
                concat!(
                    "● Hi there, five words exactly\n",
                    "\n",
                    "╭──────────────────────────────╮\n",
                    "│ > Try \"fix the lint error\"   │\n",
                    "╰──────────────────────────────╯\n",
                    "  ⏵⏵ bypass permissions on (shift+tab to cycle)\n",
                ),
                "Hi there, five words exactly",
            ),
            (
                "a gate: the tool call it is waiting on, not the question",
                concat!(
                    "⏺ Bash(rm -rf /tmp/scratch)\n",
                    "╭──────────────────────────────╮\n",
                    "│ Do you want to run this?     │\n",
                    "│ ❯ 1. Yes                     │\n",
                    "╰──────────────────────────────╯\n",
                ),
                "Bash(rm -rf /tmp/scratch)",
            ),
            (
                "an agent with no bullets: the last line above its input rule",
                concat!(
                    "compiling 12 crates\n",
                    "────────────────────\n",
                    "❯\n",
                    "────────────────────\n",
                    "  ~/repo  main  100%\n",
                ),
                "compiling 12 crates",
            ),
        ];
        for (shape, screen, expected) in cases {
            assert_eq!(
                preview_line(&lines(screen)).as_deref(),
                Some(expected),
                "{shape}"
            );
        }
    }

    #[test]
    fn preview_is_none_for_a_blank_screen() {
        assert_eq!(preview_line(&lines("\n   \n───")), None);
    }

    #[test]
    fn the_task_is_passed_as_an_argument_after_a_separator() {
        let script = launch_script(&spec("fix the flake"));
        assert_eq!(script, "cd '/repo' && exec 'claude' -- 'fix the flake'");
    }

    #[test]
    fn an_empty_task_dispatches_a_bare_interactive_session() {
        assert_eq!(launch_script(&spec("")), "cd '/repo' && exec 'claude'");
    }

    #[test]
    fn agent_and_model_reach_the_command_line() {
        let mut spec = spec("review this");
        spec.agent = Some("code-reviewer".to_owned());
        spec.model = Some("opus".to_owned());
        assert_eq!(
            launch_script(&spec),
            "cd '/repo' && exec 'claude' --agent 'code-reviewer' --model 'opus' -- 'review this'"
        );
    }

    #[test]
    fn a_quote_in_the_task_cannot_break_out_of_the_shell_word() {
        assert_eq!(quote("it's; rm -rf /"), r"'it'\''s; rm -rf /'");
    }
}
