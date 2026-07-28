//! Fleet state and every key that acts on it.
//!
//! Two modes: the list, where you type a task and dispatch it, and an attached
//! session, where every key goes to that agent's PTY instead.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui::TuiManager;
use uuid::Uuid;

use crate::keys;
use crate::session::{Session, Snapshot, Spec, Status};

/// Whether the event loop keeps going.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flow {
    Continue,
    Quit,
}

/// What the screen is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    List,
    /// Full-screen view of one session, with keys forwarded to it.
    Attached(Uuid),
    Help,
}

/// The order sections appear in, and the order [`Flow`] navigation walks: the
/// sessions that want a human first, the ones that do not last.
const SECTIONS: [Status; 3] = [Status::AwaitingInput, Status::Working, Status::Completed];

/// One session plus its activity, resolved for this frame.
pub struct Entry<'a> {
    pub session: &'a Session,
    pub snapshot: Snapshot,
}

/// Defaults applied to every dispatch, from the command line.
#[derive(Clone, Debug)]
pub struct Defaults {
    pub command: String,
    pub cwd: PathBuf,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub extra: Vec<String>,
}

pub struct App {
    manager: TuiManager,
    sessions: Vec<Session>,
    selected: Option<Uuid>,
    pub input: String,
    pub mode: Mode,
    pub message: Option<String>,
    pub defaults: Defaults,
    /// Rows and columns given to a session's PTY: the whole screen bar the
    /// attached view's footer, so attaching never resizes the agent.
    pane: (u16, u16),
}

impl App {
    pub fn new(defaults: Defaults, rows: u16, cols: u16) -> Self {
        Self {
            manager: TuiManager::new(),
            sessions: Vec::new(),
            selected: None,
            input: String::new(),
            mode: Mode::List,
            message: None,
            defaults,
            pane: pane_size(rows, cols),
        }
    }

    /// Every session with its activity, in display order.
    pub fn view(&self) -> Vec<Entry<'_>> {
        let mut entries: Vec<Entry<'_>> = self
            .sessions
            .iter()
            .map(|session| Entry {
                session,
                snapshot: session.snapshot(),
            })
            .collect();
        // Sections in a fixed order, newest first inside each one, so a
        // finishing session moves but never reshuffles its neighbours.
        entries.sort_by_key(|entry| {
            let section = SECTIONS
                .iter()
                .position(|status| *status == entry.snapshot.status)
                .unwrap_or(SECTIONS.len());
            (section, std::cmp::Reverse(entry.session.started))
        });
        entries
    }

    /// How many sessions are in each section, for the header line.
    pub fn counts(&self) -> [usize; 3] {
        let mut counts = [0; 3];
        for entry in self.view() {
            if let Some(slot) = SECTIONS
                .iter()
                .position(|status| *status == entry.snapshot.status)
                .and_then(|index| counts.get_mut(index))
            {
                *slot += 1;
            }
        }
        counts
    }

    pub fn selected(&self) -> Option<Uuid> {
        self.selected
    }

    pub fn session(&self, id: Uuid) -> Option<&Session> {
        self.sessions.iter().find(|session| session.id == id)
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Flow {
        match self.mode {
            Mode::Help => {
                self.mode = Mode::List;
                Flow::Continue
            }
            Mode::Attached(id) => {
                self.attached_key(id, key);
                Flow::Continue
            }
            Mode::List => self.list_key(key),
        }
    }

    /// Text pasted into the terminal: into the prompt on the list, straight
    /// through to the agent when attached.
    pub fn on_paste(&mut self, text: &str) {
        match self.mode {
            Mode::List => self.input.push_str(&text.replace(['\n', '\r'], " ")),
            Mode::Attached(id) => self.write(id, text),
            Mode::Help => {}
        }
    }

    /// Keep every agent's PTY the size of the screen it will be attached into.
    pub fn on_resize(&mut self, rows: u16, cols: u16) {
        let pane = pane_size(rows, cols);
        if pane == self.pane {
            return;
        }
        self.pane = pane;
        for session in &self.sessions {
            let _ = session.instance.resize(pane.0, pane.1);
        }
    }

    fn list_key(&mut self, key: KeyEvent) -> Flow {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c' | 'q') if ctrl => return Flow::Quit,
            KeyCode::Char('n') if ctrl => self.move_selection(1),
            KeyCode::Char('p') if ctrl => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('u') if ctrl => self.input.clear(),
            KeyCode::Char('w') if ctrl => drop_word(&mut self.input),
            KeyCode::Char('x') if ctrl => self.stop_selected(),
            KeyCode::Char('d') if ctrl => self.dismiss_selected(),
            KeyCode::Enter => self.submit(),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Esc => self.input.clear(),
            KeyCode::Char('?') if self.input.is_empty() => self.mode = Mode::Help,
            KeyCode::Char(c) if !ctrl => self.input.push(c),
            _ => {}
        }
        Flow::Continue
    }

    /// Every key reaches the agent except the one that leaves.
    fn attached_key(&mut self, id: Uuid, key: KeyEvent) {
        if key.code == KeyCode::Char('o') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.mode = Mode::List;
            return;
        }
        if let Some(bytes) = keys::encode(key) {
            self.write(id, &bytes);
        }
    }

    fn write(&mut self, id: Uuid, data: &str) {
        let Some(session) = self.session(id) else {
            self.mode = Mode::List;
            return;
        };
        if let Err(error) = session.instance.write(data) {
            self.message = Some(format!("write failed: {error}"));
        }
    }

    /// Enter: dispatch what is typed, or open what is selected.
    fn submit(&mut self) {
        if self.input.trim().is_empty() {
            self.input.clear();
            if let Some(id) = self.selected {
                self.mode = Mode::Attached(id);
            }
            return;
        }
        let input = std::mem::take(&mut self.input);
        self.dispatch(input.trim());
    }

    fn dispatch(&mut self, input: &str) {
        let task = Task::parse(input, self.defaults.agent.clone());
        let spec = Spec {
            command: self.defaults.command.clone(),
            cwd: self.defaults.cwd.clone(),
            task: task.text,
            agent: task.agent,
            model: self.defaults.model.clone(),
            extra: self.defaults.extra.clone(),
        };
        match Session::spawn(&self.manager, &spec, self.pane.0, self.pane.1) {
            Ok(session) => {
                self.selected = Some(session.id);
                self.sessions.push(session);
                self.message = None;
            }
            Err(error) => self.message = Some(format!("could not start {}: {error}", spec.command)),
        }
    }

    /// Kill the selected agent. Its final screen stays in the list until it is
    /// dismissed, which is the only record of how it ended.
    fn stop_selected(&mut self) {
        let Some(session) = self.selected.and_then(|id| self.session(id)) else {
            return;
        };
        match session.instance.kill() {
            Ok(()) => self.message = Some(format!("stopped {}", session.title())),
            Err(error) => self.message = Some(format!("could not stop: {error}")),
        }
    }

    /// Drop a finished session off the list. Refuses while it is still running,
    /// so nothing disappears without having been stopped first.
    fn dismiss_selected(&mut self) {
        let Some(id) = self.selected else { return };
        let Some(index) = self.sessions.iter().position(|s| s.id == id) else {
            return;
        };
        let Some(session) = self.sessions.get(index) else {
            return;
        };
        if session.snapshot().status != Status::Completed {
            self.message = Some("still running: ctrl-x stops it first".to_owned());
            return;
        }
        self.select_neighbour(id);
        let _ = self.manager.remove(&id);
        self.sessions.remove(index);
    }

    /// Move the selection off `id` before it leaves the list.
    fn select_neighbour(&mut self, id: Uuid) {
        let order: Vec<Uuid> = self.view().iter().map(|entry| entry.session.id).collect();
        let position = order.iter().position(|other| *other == id);
        self.selected = position.and_then(|index| {
            order
                .get(index + 1)
                .or_else(|| index.checked_sub(1).and_then(|prev| order.get(prev)))
                .copied()
        });
    }

    fn move_selection(&mut self, delta: isize) {
        let order: Vec<Uuid> = self.view().iter().map(|entry| entry.session.id).collect();
        if order.is_empty() {
            self.selected = None;
            return;
        }
        let current = self
            .selected
            .and_then(|id| order.iter().position(|other| *other == id));
        let next = match current {
            // Saturating rather than wrapping: holding ctrl-n parks on the last
            // row instead of silently looping back to the top.
            Some(index) => index.saturating_add_signed(delta).min(order.len() - 1),
            None if delta < 0 => order.len() - 1,
            None => 0,
        };
        self.selected = order.get(next).copied();
    }

    /// Stop every agent. Called on the way out: a PTY whose master closes
    /// leaves the child orphaned, so the exit is explicit instead.
    pub fn shutdown(&mut self) {
        for session in &self.sessions {
            let _ = session.instance.kill();
        }
    }
}

/// A dispatch line split into the agent it names and the task itself.
struct Task {
    text: String,
    agent: Option<String>,
}

impl Task {
    /// `+reviewer check the diff` runs under `--agent reviewer`; anything else
    /// is the whole line, dispatched to the default agent.
    fn parse(input: &str, default_agent: Option<String>) -> Self {
        let input = input.trim();
        if let Some(rest) = input.strip_prefix('+')
            && let Some((name, task)) = rest.split_once(char::is_whitespace)
            && !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Self {
                text: task.trim().to_owned(),
                agent: Some(name.to_owned()),
            };
        }
        Self {
            text: input.to_owned(),
            agent: default_agent,
        }
    }
}

/// Drop the last whitespace-separated word, the readline `ctrl-w`.
fn drop_word(input: &mut String) {
    let trimmed = input.trim_end();
    let cut = trimmed.rfind(char::is_whitespace).map_or(0, |at| at + 1);
    input.truncate(cut);
}

/// A session's PTY is the screen minus the attached view's footer row.
fn pane_size(rows: u16, cols: u16) -> (u16, u16) {
    (rows.saturating_sub(1).max(1), cols.max(1))
}

#[cfg(test)]
mod tests {
    use super::{Task, drop_word, pane_size};

    #[test]
    fn a_leading_plus_names_the_agent() {
        let task = Task::parse("+code-reviewer look at the diff", None);
        assert_eq!(task.agent.as_deref(), Some("code-reviewer"));
        assert_eq!(task.text, "look at the diff");
    }

    #[test]
    fn a_plus_inside_the_task_is_just_text() {
        let task = Task::parse("add a + b", Some("claude".to_owned()));
        assert_eq!(task.agent.as_deref(), Some("claude"));
        assert_eq!(task.text, "add a + b");
    }

    #[test]
    fn a_bare_plus_word_is_not_an_agent_because_it_leaves_no_task() {
        let task = Task::parse("+reviewer", None);
        assert_eq!(task.agent, None);
        assert_eq!(task.text, "+reviewer");
    }

    #[test]
    fn the_command_line_agent_is_the_default() {
        let task = Task::parse("fix it", Some("explore".to_owned()));
        assert_eq!(task.agent.as_deref(), Some("explore"));
    }

    #[test]
    fn ctrl_w_drops_one_word_at_a_time() {
        let mut input = String::from("fix the flake ");
        drop_word(&mut input);
        assert_eq!(input, "fix the ");
        drop_word(&mut input);
        assert_eq!(input, "fix ");
        drop_word(&mut input);
        assert_eq!(input, "");
    }

    #[test]
    fn a_pane_never_collapses_to_zero() {
        assert_eq!(pane_size(1, 80), (1, 80));
        assert_eq!(pane_size(0, 0), (1, 1));
        assert_eq!(pane_size(40, 120), (39, 120));
    }
}
