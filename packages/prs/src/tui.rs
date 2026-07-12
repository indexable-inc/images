//! Interactive ratatui front end: a vim-flavored table over the patch rows.
//!
//! Keys: `j`/`k` (or arrows) move, `gg`/`G` jump, `/` filters, `Enter`/`o`
//! opens the selected patch's PR in the browser, `e` edits the patch in
//! `$EDITOR`, `E` opens the patch's directory there, `d` previews the diff
//! in a scrollable overlay, `y` copies the PR URL to the clipboard (OSC 52),
//! `r` re-fetches PR status, `?` shows help, `Esc` clears the filter, `q`
//! quits.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use base64::Engine as _;
use color_eyre::eyre::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState};
use ratatui::{DefaultTerminal, Frame};

use crate::model::{CiState, PatchRow, PrRef, PrSource, PrState, ReviewState};
use crate::{github, plain};

/// Fallback diff-preview page size before the first draw measures the real
/// viewport.
const DEFAULT_PAGE: usize = 20;

pub fn run(rows: Vec<PatchRow>, note: Option<String>, token: Option<String>) -> Result<()> {
    let mut terminal = ratatui::init();
    let mut app = App::new(rows, note, token);
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

/// Draw/read/dispatch until quit. Split from [`run`] so `?` propagation still
/// reaches the `ratatui::restore` that un-breaks the caller's terminal.
fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match app.handle_key(key) {
            Action::None => {}
            Action::Quit => return Ok(()),
            Action::Edit(path) => app.message = edit(terminal, &path),
            Action::Copy(text) => app.message = copy_to_clipboard(&text),
            Action::Refresh => {
                "refreshing PR status from GitHub...".clone_into(&mut app.message);
                terminal.draw(|frame| app.draw(frame))?;
                app.message = app.refresh();
            }
        }
    }
}

/// What the event loop must do after a key press. Everything that needs the
/// terminal or blocks (editor, clipboard escape, network) is routed through
/// here so [`App::handle_key`] stays a pure, testable state transition.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    None,
    Quit,
    /// Suspend the TUI and open this path in the user's editor.
    Edit(PathBuf),
    /// Send this text to the clipboard via an OSC 52 escape.
    Copy(String),
    /// Re-fetch PR statuses.
    Refresh,
}

/// A modal pane drawn over the table; keys go to it while it is up.
enum Overlay {
    None,
    /// Key reference; any key closes it.
    Help,
    /// Scrollable patch preview (`d`), one entry per line of the file.
    Diff {
        title: String,
        lines: Vec<String>,
    },
}

struct App {
    rows: Vec<PatchRow>,
    /// Indices into `rows` that pass the current filter.
    visible: Vec<usize>,
    table: TableState,
    filter: String,
    filter_active: bool,
    /// A `g` was pressed and awaits the second `g` of `gg`.
    pending_g: bool,
    message: String,
    /// GitHub token for the `r` re-fetch; `None` means offline / no token.
    token: Option<String>,
    overlay: Overlay,
    /// Scroll offset of the diff overlay, in lines.
    diff_scroll: usize,
    /// Diff-overlay viewport height as of the last draw; sizes `ctrl-d`/`u`.
    diff_height: usize,
}

impl App {
    fn new(rows: Vec<PatchRow>, note: Option<String>, token: Option<String>) -> Self {
        let mut app = Self {
            visible: (0..rows.len()).collect(),
            rows,
            table: TableState::default(),
            filter: String::new(),
            filter_active: false,
            pending_g: false,
            message: note.unwrap_or_default(),
            token,
            overlay: Overlay::None,
            diff_scroll: 0,
            diff_height: DEFAULT_PAGE,
        };
        app.table.select(app.visible.first().map(|_| 0));
        app
    }

    fn refilter(&mut self) {
        self.visible = (0..self.rows.len())
            .filter(|&index| self.filter.is_empty() || self.rows[index].matches(&self.filter))
            .collect();
        let last = self.visible.len().saturating_sub(1);
        match self.table.selected() {
            Some(selected) if !self.visible.is_empty() => {
                self.table.select(Some(selected.min(last)));
            }
            _ => self.table.select(self.visible.first().map(|_| 0)),
        }
    }

    fn move_by(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let last = self.visible.len() - 1;
        let current = self.table.selected().unwrap_or(0);
        let next = current.saturating_add_signed(delta).min(last);
        self.table.select(Some(next));
    }

    fn selected_row(&self) -> Option<&PatchRow> {
        let index = *self.visible.get(self.table.selected()?)?;
        self.rows.get(index)
    }

    fn open_selected(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        let Some(pr) = &row.pr else {
            self.message = format!("{}: no PR associated with this patch", row.file);
            return;
        };
        let url = pr.url.clone();
        self.message = match open_in_browser(&url) {
            Ok(()) => format!("opened {url}"),
            Err(error) => format!("could not open {url}: {error}"),
        };
    }

    /// `e` (`Target::Patch`): the patch file itself, falling back to the
    /// series directory when the file is not on disk. `E` (`Target::Dir`):
    /// always the directory.
    fn edit_selected(&mut self, target: Target) -> Action {
        let Some(row) = self.selected_row() else {
            return Action::None;
        };
        let file = row.file.clone();
        let path = match target {
            Target::Patch => row.path.clone().or_else(|| row.dir.clone()),
            Target::Dir => row.dir.clone().or_else(|| row.path.clone()),
        };
        let Some(path) = path else {
            self.message = format!("{file}: not on disk (run inside the repo checkout)");
            return Action::None;
        };
        Action::Edit(path)
    }

    /// `y`: hand the selected PR URL to the event loop for an OSC 52 copy.
    fn yank_selected(&mut self) -> Action {
        let Some(row) = self.selected_row() else {
            return Action::None;
        };
        if let Some(pr) = &row.pr {
            return Action::Copy(pr.url.clone());
        }
        let message = format!("{}: no PR associated with this patch", row.file);
        self.message = message;
        Action::None
    }

    /// `d`: load the selected patch into the scrollable diff overlay.
    fn preview_selected(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        let file = row.file.clone();
        let Some(path) = row.path.clone() else {
            self.message = format!("{file}: not on disk (run inside the repo checkout)");
            return;
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                self.overlay = Overlay::Diff {
                    title: file,
                    lines: text.lines().map(str::to_owned).collect(),
                };
                self.diff_scroll = 0;
            }
            Err(error) => self.message = format!("could not read {}: {error}", path.display()),
        }
    }

    /// `r`: blocking status re-fetch; the event loop draws a "refreshing"
    /// frame first. Rows keep their order, so the selection is untouched.
    fn refresh(&mut self) -> String {
        let Some(token) = self.token.clone() else {
            return "cannot refresh: offline or no GitHub token".to_owned();
        };
        let prs: Vec<PrRef> = self.rows.iter().filter_map(|row| row.pr.clone()).collect();
        if prs.is_empty() {
            return "no patch references an upstream PR yet".to_owned();
        }
        match github::fetch(&prs, &token) {
            Ok(statuses) => {
                for row in &mut self.rows {
                    row.status = row
                        .pr
                        .as_ref()
                        .and_then(|pr| statuses.get(&pr.url))
                        .cloned();
                }
                format!("refreshed status for {} PRs", statuses.len())
            }
            Err(error) => format!("refresh failed: {error}"),
        }
    }

    /// Dispatch one key press; the returned [`Action`] is what the event loop
    /// must carry out.
    fn handle_key(&mut self, key: KeyEvent) -> Action {
        if !matches!(self.overlay, Overlay::None) {
            self.handle_overlay_key(key);
            return Action::None;
        }
        if self.filter_active {
            match key.code {
                KeyCode::Esc => {
                    self.filter.clear();
                    self.filter_active = false;
                    self.refilter();
                }
                KeyCode::Enter => self.filter_active = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.refilter();
                }
                KeyCode::Char(c) => {
                    self.filter.push(c);
                    self.refilter();
                }
                _ => {}
            }
            return Action::None;
        }
        let pending_g = std::mem::take(&mut self.pending_g);
        match key.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('j') | KeyCode::Down => self.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_by(-1),
            KeyCode::Char('g') if pending_g => self.table.select(self.visible.first().map(|_| 0)),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') | KeyCode::End => {
                if !self.visible.is_empty() {
                    self.table.select(Some(self.visible.len() - 1));
                }
            }
            KeyCode::Home => self.table.select(self.visible.first().map(|_| 0)),
            KeyCode::Char('/') => {
                self.filter_active = true;
                self.message.clear();
            }
            KeyCode::Esc => {
                self.filter.clear();
                self.refilter();
            }
            KeyCode::Enter | KeyCode::Char('o') => self.open_selected(),
            KeyCode::Char('e') => return self.edit_selected(Target::Patch),
            KeyCode::Char('E') => return self.edit_selected(Target::Dir),
            KeyCode::Char('d') => self.preview_selected(),
            KeyCode::Char('y') => return self.yank_selected(),
            KeyCode::Char('r') => return Action::Refresh,
            KeyCode::Char('?') => self.overlay = Overlay::Help,
            _ => {}
        }
        Action::None
    }

    /// Keys while an overlay is up: help closes on anything; the diff scrolls
    /// vim-style until `q`/`Esc`.
    fn handle_overlay_key(&mut self, key: KeyEvent) {
        if matches!(self.overlay, Overlay::Help) {
            self.overlay = Overlay::None;
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let half_page = (self.diff_height / 2).max(1);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.overlay = Overlay::None;
                self.diff_scroll = 0;
            }
            KeyCode::Char('d' | 'D') if ctrl => self.scroll_diff_down(half_page),
            KeyCode::Char('u' | 'U') if ctrl => self.scroll_diff_up(half_page),
            KeyCode::Char('j') | KeyCode::Down => self.scroll_diff_down(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_diff_up(1),
            KeyCode::PageDown => self.scroll_diff_down(self.diff_height.max(1)),
            KeyCode::PageUp => self.scroll_diff_up(self.diff_height.max(1)),
            KeyCode::Char('g') | KeyCode::Home => self.diff_scroll = 0,
            KeyCode::Char('G') | KeyCode::End => self.scroll_diff_down(usize::MAX),
            _ => {}
        }
    }

    /// Furthest useful scroll: the last viewport-full of the diff.
    fn max_diff_scroll(&self) -> usize {
        match &self.overlay {
            Overlay::Diff { lines, .. } => lines.len().saturating_sub(self.diff_height.max(1)),
            _ => 0,
        }
    }

    fn scroll_diff_down(&mut self, lines: usize) {
        self.diff_scroll = self
            .diff_scroll
            .saturating_add(lines)
            .min(self.max_diff_scroll());
    }

    const fn scroll_diff_up(&mut self, lines: usize) {
        self.diff_scroll = self.diff_scroll.saturating_sub(lines);
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [table_area, footer_area] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
        self.draw_table(frame, table_area);
        self.draw_footer(frame, footer_area);
        match self.overlay {
            Overlay::None => {}
            Overlay::Help => draw_help(frame, table_area),
            Overlay::Diff { .. } => self.draw_diff(frame, table_area),
        }
    }

    fn draw_table(&mut self, frame: &mut Frame, area: Rect) {
        let header = Row::new(
            [
                "FORK", "PATCH", "INTENT", "PR", "STATE", "CI", "REVIEW", "UNRES",
            ]
            .map(|title| Cell::from(title.bold())),
        )
        .style(Style::new().add_modifier(Modifier::UNDERLINED));
        let rows = self.visible.iter().map(|&index| {
            let row = &self.rows[index];
            let cells = plain::cells(row);
            let status = row.status.as_ref();
            Row::new([
                Cell::from(cells[0].clone()),
                Cell::from(cells[1].clone()),
                Cell::from(Span::styled(cells[2].clone(), intent_style(row))),
                Cell::from(cells[3].clone()),
                Cell::from(Span::styled(
                    cells[4].clone(),
                    status.map_or_else(Style::new, |status| state_style(status.state)),
                )),
                Cell::from(Span::styled(
                    cells[5].clone(),
                    status
                        .and_then(|status| status.ci)
                        .map_or_else(Style::new, ci_style),
                )),
                Cell::from(Span::styled(
                    cells[6].clone(),
                    status
                        .and_then(|status| status.review)
                        .map_or_else(Style::new, review_style),
                )),
                Cell::from(cells[7].clone()),
            ])
        });
        let title = format!(
            " vendored-dependency patches ({}/{}) ",
            self.visible.len(),
            self.rows.len()
        );
        let table = Table::new(
            rows,
            [
                Constraint::Length(24),
                Constraint::Min(30),
                Constraint::Length(7),
                Constraint::Length(28),
                Constraint::Length(6),
                Constraint::Length(7),
                Constraint::Length(8),
                Constraint::Length(5),
            ],
        )
        .header(header)
        .block(Block::new().borders(Borders::TOP).title(title))
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
        frame.render_stateful_widget(table, area, &mut self.table);
    }

    /// The diff overlay: the whole table area, cleared, with the visible
    /// window of the patch inside a bordered block.
    fn draw_diff(&mut self, frame: &mut Frame, area: Rect) {
        let Overlay::Diff { title, lines } = &self.overlay else {
            return;
        };
        let block = Block::bordered().title(format!(" {title} "));
        let body = block.inner(area);
        self.diff_height = usize::from(body.height);
        let max = lines.len().saturating_sub(self.diff_height.max(1));
        self.diff_scroll = self.diff_scroll.min(max);
        let text: Vec<Line> = lines
            .iter()
            .skip(self.diff_scroll)
            .take(self.diff_height)
            .map(|line| Line::from(Span::styled(line.clone(), diff_style(line))))
            .collect();
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(text).block(block), area);
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let line = if matches!(self.overlay, Overlay::Diff { .. }) {
            Line::from(Span::styled(
                " j/k scroll  ctrl-d/u half page  g/G top/bottom  q/Esc close ",
                Style::new().fg(Color::DarkGray),
            ))
        } else if self.filter_active {
            Line::from(vec![
                Span::styled("/", Style::new().fg(Color::Yellow)),
                Span::raw(self.filter.clone()),
                Span::styled("_", Style::new().add_modifier(Modifier::SLOW_BLINK)),
            ])
        } else if self.message.is_empty() {
            let mut spans = vec![Span::styled(
                " j/k move  / filter  Enter open PR  e edit  d diff  y yank  r refresh  ? help  q quit ",
                Style::new().fg(Color::DarkGray),
            )];
            if !self.filter.is_empty() {
                spans.push(Span::styled(
                    format!(" filter: {} (Esc clears)", self.filter),
                    Style::new().fg(Color::Yellow),
                ));
            }
            if let Some(source) = self.selected_row().and_then(|row| row.pr_source) {
                let via = match source {
                    PrSource::Mapping => "pr field in lib/fork-packages.nix",
                    PrSource::Status => "upstream-status.json",
                    PrSource::PatchHeader => "patch header",
                };
                spans.push(Span::styled(
                    format!(" PR via {via}"),
                    Style::new().fg(Color::DarkGray),
                ));
            }
            Line::from(spans)
        } else {
            Line::from(Span::styled(
                format!(" {} ", self.message),
                Style::new().fg(Color::Cyan),
            ))
        };
        frame.render_widget(line, area);
    }
}

/// What `e`/`E` should open.
#[derive(Debug, Clone, Copy)]
enum Target {
    Patch,
    Dir,
}

/// Key/description pairs for the `?` overlay.
const HELP: [(&str, &str); 11] = [
    ("j/k, arrows", "move the selection"),
    ("gg / G", "jump to the first / last row"),
    ("/", "filter rows (Enter keeps it, Esc clears it)"),
    ("Enter / o", "open the selected PR in the browser"),
    ("e", "edit the patch in $EDITOR ($VISUAL, vi)"),
    ("E", "open the patch's directory in the editor"),
    ("d", "preview the patch diff (j/k, ctrl-d/u scroll)"),
    ("y", "copy the PR URL to the clipboard (OSC 52)"),
    ("r", "refresh PR status from GitHub"),
    ("?", "this help"),
    ("q", "quit"),
];

/// Centered key-reference overlay; [`App::handle_overlay_key`] closes it on
/// any key.
fn draw_help(frame: &mut Frame, area: Rect) {
    let key_width = HELP
        .iter()
        .map(|(key, _)| key.len())
        .max()
        .unwrap_or_default();
    let lines: Vec<Line> = HELP
        .iter()
        .map(|&(key, description)| {
            Line::from(vec![
                Span::styled(format!(" {key:>key_width$}  "), Style::new().bold()),
                Span::raw(description),
            ])
        })
        .collect();
    let height = u16::try_from(lines.len())
        .expect("help fits in u16")
        .saturating_add(2)
        .min(area.height);
    let width = area.width.min(64);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" keys (any key closes) ")),
        popup,
    );
}

fn intent_style(row: &PatchRow) -> Style {
    match row.intent.as_deref() {
        Some("attempt") => Style::new().fg(Color::Green),
        Some("hold") => Style::new().fg(Color::Yellow),
        Some("never") => Style::new().fg(Color::DarkGray),
        _ => Style::new(),
    }
}

const fn state_style(state: PrState) -> Style {
    match state {
        PrState::Open => Style::new().fg(Color::Green),
        PrState::Draft => Style::new().fg(Color::DarkGray),
        PrState::Merged => Style::new().fg(Color::Magenta),
        PrState::Closed => Style::new().fg(Color::Red),
    }
}

const fn ci_style(ci: CiState) -> Style {
    match ci {
        CiState::Passing => Style::new().fg(Color::Green),
        CiState::Failing => Style::new().fg(Color::Red),
        CiState::Pending => Style::new().fg(Color::Yellow),
    }
}

const fn review_style(review: ReviewState) -> Style {
    match review {
        ReviewState::Approved => Style::new().fg(Color::Green),
        ReviewState::ChangesRequested => Style::new().fg(Color::Red),
        ReviewState::ReviewRequired => Style::new().fg(Color::Yellow),
    }
}

/// Unified-diff line coloring for the preview overlay.
fn diff_style(line: &str) -> Style {
    if line.starts_with("+++") || line.starts_with("---") || line.starts_with("diff --git") {
        Style::new().add_modifier(Modifier::BOLD)
    } else if line.starts_with('+') {
        Style::new().fg(Color::Green)
    } else if line.starts_with('-') {
        Style::new().fg(Color::Red)
    } else if line.starts_with("@@") {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new()
    }
}

/// The user's editor: `$EDITOR`, else `$VISUAL`, else `vi`. Extra words in
/// the variable become leading arguments (`EDITOR="code --wait"`).
struct Editor {
    program: String,
    args: Vec<String>,
}

impl Editor {
    fn from_env() -> Self {
        let raw = ["EDITOR", "VISUAL"]
            .iter()
            .find_map(|var| std::env::var(*var).ok())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "vi".to_owned());
        let mut words = raw.split_whitespace().map(str::to_owned);
        Self {
            program: words.next().unwrap_or_else(|| "vi".to_owned()),
            args: words.collect(),
        }
    }
}

/// Suspend the TUI (leave the alternate screen, restore the cooked terminal),
/// run the editor on `path` with the real terminal, then re-enter the TUI.
/// The fresh `ratatui::init` starts from an empty back buffer, so the next
/// draw repaints everything the editor left behind. Returns the footer
/// message.
fn edit(terminal: &mut DefaultTerminal, path: &Path) -> String {
    let editor = Editor::from_env();
    ratatui::restore();
    let status = Command::new(&editor.program)
        .args(&editor.args)
        .arg(path)
        .status();
    *terminal = ratatui::init();
    match status {
        Ok(status) if status.success() => format!("edited {}", path.display()),
        Ok(status) => format!("{} exited with {status}", editor.program),
        Err(error) => format!("could not run {}: {error}", editor.program),
    }
}

/// Best-effort OSC 52 clipboard write. The escape rides the normal output
/// stream, so it works across ssh wherever the terminal supports it; a
/// terminal that does not simply ignores it. Returns the footer message.
fn copy_to_clipboard(text: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let mut stdout = std::io::stdout().lock();
    match write!(stdout, "\x1b]52;c;{encoded}\x07").and_then(|()| stdout.flush()) {
        Ok(()) => format!("copied {text}"),
        Err(error) => format!("could not copy: {error}"),
    }
}

/// Open a URL with the platform opener, detached from the TUI's terminal.
fn open_in_browser(url: &str) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    Command::new(opener)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{Action, App, Overlay};
    use crate::model::PatchRow;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn press(app: &mut App, code: KeyCode) -> Action {
        app.handle_key(key(code))
    }

    fn row(fork: &str, file: &str, pr_url: Option<&str>) -> PatchRow {
        PatchRow {
            fork: fork.to_owned(),
            file: file.to_owned(),
            intent: None,
            pr: pr_url.and_then(crate::discover::parse_pr_url),
            pr_source: None,
            status: None,
            path: None,
            dir: None,
        }
    }

    fn app(rows: Vec<PatchRow>) -> App {
        App::new(rows, None, None)
    }

    fn three_row_app() -> App {
        app(vec![
            row("alpha", "0001-a.patch", None),
            row("beta", "0001-b.patch", None),
            row("gamma", "0001-c.patch", None),
        ])
    }

    #[test]
    fn movement_and_quit() {
        let mut app = three_row_app();
        assert_eq!(press(&mut app, KeyCode::Char('j')), Action::None);
        assert_eq!(app.table.selected(), Some(1));
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.table.selected(), Some(2));
        press(&mut app, KeyCode::Char('g'));
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.table.selected(), Some(0));
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.table.selected(), Some(0));
        assert_eq!(press(&mut app, KeyCode::Char('q')), Action::Quit);
    }

    #[test]
    fn filter_narrows_and_captures_command_keys() {
        let mut app = three_row_app();
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('b'));
        assert_eq!(app.visible, vec![1]);
        // While the filter prompt is live, `q` is text, not quit.
        assert_eq!(press(&mut app, KeyCode::Char('q')), Action::None);
        assert!(app.visible.is_empty());
        press(&mut app, KeyCode::Esc);
        assert!(!app.filter_active);
        assert_eq!(app.visible.len(), 3);
    }

    #[test]
    fn help_overlay_opens_and_any_key_closes() {
        let mut app = three_row_app();
        press(&mut app, KeyCode::Char('?'));
        assert!(matches!(app.overlay, Overlay::Help));
        // Keys go to the overlay, not the table.
        assert_eq!(press(&mut app, KeyCode::Char('j')), Action::None);
        assert!(matches!(app.overlay, Overlay::None));
        assert_eq!(app.table.selected(), Some(0));
    }

    #[test]
    fn yank_returns_copy_action_with_pr_url() {
        let url = "https://github.com/nushell/nushell/pull/18549";
        let mut app = app(vec![row("alpha", "0001-a.patch", Some(url))]);
        assert_eq!(
            press(&mut app, KeyCode::Char('y')),
            Action::Copy(url.to_owned())
        );
    }

    #[test]
    fn yank_without_pr_reports_instead() {
        let mut app = three_row_app();
        assert_eq!(press(&mut app, KeyCode::Char('y')), Action::None);
        assert!(app.message.contains("no PR"));
    }

    #[test]
    fn edit_prefers_patch_file_and_falls_back_to_dir() {
        let mut app = three_row_app();
        app.rows[0].path = Some(PathBuf::from("/repo/series/0001-a.patch"));
        app.rows[0].dir = Some(PathBuf::from("/repo/series"));
        assert_eq!(
            press(&mut app, KeyCode::Char('e')),
            Action::Edit(PathBuf::from("/repo/series/0001-a.patch"))
        );
        assert_eq!(
            press(&mut app, KeyCode::Char('E')),
            Action::Edit(PathBuf::from("/repo/series"))
        );
        // A mapping-only row with just a directory: `e` opens the directory.
        app.rows[0].path = None;
        assert_eq!(
            press(&mut app, KeyCode::Char('e')),
            Action::Edit(PathBuf::from("/repo/series"))
        );
    }

    #[test]
    fn edit_without_any_path_reports_instead() {
        let mut app = three_row_app();
        assert_eq!(press(&mut app, KeyCode::Char('e')), Action::None);
        assert!(app.message.contains("not on disk"));
    }

    #[test]
    fn refresh_key_and_offline_refresh_message() {
        let mut app = three_row_app();
        assert_eq!(press(&mut app, KeyCode::Char('r')), Action::Refresh);
        assert!(app.refresh().contains("offline or no GitHub token"));
    }

    #[test]
    fn diff_overlay_scrolls_and_closes() {
        let dir = std::env::temp_dir().join(format!("prs-tui-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("0001-a.patch");
        let body = (0..100).fold(String::new(), |mut text, index| {
            use std::fmt::Write as _;
            let _ = writeln!(text, "+line {index}");
            text
        });
        std::fs::write(&path, body).expect("write patch");

        let mut app = three_row_app();
        app.rows[0].path = Some(path);
        app.diff_height = 10;
        press(&mut app, KeyCode::Char('d'));
        assert!(matches!(&app.overlay, Overlay::Diff { lines, .. } if lines.len() == 100));

        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.diff_scroll, 1);
        app.handle_key(ctrl('d'));
        assert_eq!(app.diff_scroll, 6);
        app.handle_key(ctrl('u'));
        assert_eq!(app.diff_scroll, 1);
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.diff_scroll, 90);
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.diff_scroll, 0);
        // The table selection never moved while the overlay had the keys.
        assert_eq!(app.table.selected(), Some(0));
        press(&mut app, KeyCode::Char('q'));
        assert!(matches!(app.overlay, Overlay::None));

        std::fs::remove_dir_all(&dir).expect("clean temp dir");
    }

    #[test]
    fn diff_preview_without_file_reports_instead() {
        let mut app = three_row_app();
        press(&mut app, KeyCode::Char('d'));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.message.contains("not on disk"));
    }
}
