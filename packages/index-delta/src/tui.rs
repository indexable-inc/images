//! `index-delta tui`: an interactive drift browser. The sidebar lists every
//! pending file; the detail pane renders each side's diff (base vs the file
//! on disk, plus base vs the staged incoming base for conflicts) in its
//! format's model: logical ops for structured formats, a unified line diff
//! for text. Enter suspends into `$VISUAL`/`$EDITOR` on the selected file
//! and re-diffs on return.

use std::env;
use std::io::{self, Stdout};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph},
};
use similar::{ChangeTag, TextDiff};

use crate::cmd::{State, TuiDiff, TuiEntry, tui_entries};
use crate::diff::Op;
use crate::store::{Persistence, Store};

type TuiTerminal = Terminal<CrosstermBackend<Stdout>>;

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;

struct Badge {
    icon: &'static str,
    label: &'static str,
    color: Color,
}

const fn state_badge(state: State) -> Badge {
    match state {
        State::Clean => Badge {
            icon: "✓",
            label: "clean",
            color: Color::Green,
        },
        State::Drifted => Badge {
            icon: "~",
            label: "drifted",
            color: Color::Yellow,
        },
        State::Conflict => Badge {
            icon: "✗",
            label: "conflict",
            color: Color::Red,
        },
        State::Snoozed => Badge {
            icon: "·",
            label: "snoozed",
            color: DIM,
        },
    }
}

struct Entry {
    path: String,
    state: State,
    detail: Vec<Line<'static>>,
}

struct App {
    entries: Vec<Entry>,
    selected: usize,
    scroll: u16,
    message: Option<String>,
}

impl App {
    fn load(store: &Store) -> Result<Self> {
        let entries = tui_entries(store)?
            .iter()
            .map(|entry| Entry {
                path: entry.path.clone(),
                state: entry.state,
                detail: detail_lines(entry),
            })
            .collect();
        Ok(Self {
            entries,
            selected: 0,
            scroll: 0,
            message: None,
        })
    }

    /// Re-read the store (after an edit) keeping the selection on the same
    /// path when it is still pending.
    fn reload(&mut self, store: &Store) -> Result<()> {
        let path = self
            .entries
            .get(self.selected)
            .map(|entry| entry.path.clone());
        self.entries = Self::load(store)?.entries;
        self.selected = path
            .and_then(|path| self.entries.iter().position(|entry| entry.path == path))
            .unwrap_or(0)
            .min(self.entries.len().saturating_sub(1));
        self.scroll = self.scroll.min(self.max_scroll());
        Ok(())
    }

    fn select(&mut self, index: usize) {
        self.selected = index.min(self.entries.len().saturating_sub(1));
        self.scroll = 0;
    }

    fn max_scroll(&self) -> u16 {
        let lines = self
            .entries
            .get(self.selected)
            .map_or(0, |entry| entry.detail.len().saturating_sub(1));
        u16::try_from(lines.min(usize::from(u16::MAX))).expect("capped at u16::MAX")
    }

    fn scroll_down(&mut self, lines: u16) {
        self.scroll = self.scroll.saturating_add(lines).min(self.max_scroll());
    }

    const fn scroll_up(&mut self, lines: u16) {
        self.scroll = self.scroll.saturating_sub(lines);
    }
}

pub fn run(store: &Store) -> Result<()> {
    let app = App::load(store)?;
    let mut terminal = init_terminal()?;
    let result = run_loop(&mut terminal, store, app);
    restore_terminal(&mut terminal)?;
    result
}

fn init_terminal() -> Result<TuiTerminal> {
    enable_raw_mode()?;
    // From here on a failure must unwind the terminal state already taken,
    // best-effort so cleanup never shadows the root error.
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(error.into());
    }
    Terminal::new(CrosstermBackend::new(stdout)).map_err(|error| {
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
        error.into()
    })
}

fn restore_terminal(terminal: &mut TuiTerminal) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn run_loop(terminal: &mut TuiTerminal, store: &Store, mut app: App) -> Result<()> {
    loop {
        terminal.draw(|frame| render(frame, &app))?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        app.message = None;
        match (key.code, key.modifiers) {
            (KeyCode::Char('q') | KeyCode::Esc, _) => return Ok(()),
            (KeyCode::Char('j') | KeyCode::Down, _) => app.select(app.selected + 1),
            (KeyCode::Char('k') | KeyCode::Up, _) => app.select(app.selected.saturating_sub(1)),
            (KeyCode::Char('g') | KeyCode::Home, _) => app.select(0),
            (KeyCode::Char('G') | KeyCode::End, _) => app.select(usize::MAX),
            (KeyCode::Char('J'), _) => app.scroll_down(1),
            (KeyCode::Char('K'), _) => app.scroll_up(1),
            (KeyCode::Char('d'), KeyModifiers::CONTROL) | (KeyCode::PageDown, _) => {
                app.scroll_down(12);
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) | (KeyCode::PageUp, _) => {
                app.scroll_up(12);
            }
            (KeyCode::Char('r'), _) => app.reload(store)?,
            (KeyCode::Enter, _) => {
                let Some(entry) = app.entries.get(app.selected) else {
                    continue;
                };
                let path = entry.path.clone();
                match edit(terminal, &path)? {
                    EditOutcome::Edited => app.reload(store)?,
                    EditOutcome::EditorFailed(error) => app.message = Some(format!("{error:#}")),
                }
            }
            _ => {}
        }
    }
}

/// The editor round-trip's two failure severities: the editor itself
/// failing is recoverable (shown in the footer), while a terminal that
/// could not be suspended or resumed leaves raw mode and the alternate
/// screen in an unknown state, so those errors propagate and end the TUI
/// through the normal restore path.
enum EditOutcome {
    Edited,
    EditorFailed(anyhow::Error),
}

/// Suspend the TUI, run the user's editor on `path`, and resume.
fn edit(terminal: &mut TuiTerminal, path: &str) -> Result<EditOutcome> {
    // Set-but-empty counts as unset, matching git's editor resolution.
    let Some(editor) = ["VISUAL", "EDITOR"]
        .into_iter()
        .find_map(|name| env::var(name).ok().filter(|value| !value.is_empty()))
    else {
        return Ok(EditOutcome::EditorFailed(anyhow!(
            "neither $VISUAL nor $EDITOR is set"
        )));
    };
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    // $EDITOR is a shell fragment by convention (it may carry flags), so it
    // must go through a shell; the path rides as "$1" and never needs
    // escaping.
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("index-delta-edit")
        .arg(path)
        .status();
    enable_raw_mode().context("resuming after editor")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("resuming after editor")?;
    // Rebuild instead of `Terminal::clear()`: clear does a cursor-position
    // DSR round-trip (ESC[6n) that times out on terminals that never reply,
    // leaving the old buffers intact and the screen blank. A fresh Terminal
    // starts with empty buffers, so the next draw repaints every cell.
    *terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    Ok(match status.context("launching editor") {
        Ok(status) if status.success() => EditOutcome::Edited,
        Ok(status) => EditOutcome::EditorFailed(anyhow!("editor exited with {status}")),
        Err(error) => EditOutcome::EditorFailed(error),
    })
}

// --- rendering ---

fn render(frame: &mut ratatui::Frame, app: &App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    render_header(frame, header, app);
    let [sidebar, detail] =
        Layout::horizontal([Constraint::Percentage(34), Constraint::Percentage(66)]).areas(body);
    render_sidebar(frame, sidebar, app);
    render_detail(frame, detail, app);
    render_footer(frame, footer, app);
}

fn render_header(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let mut spans = vec![Span::styled(
        " index-delta ",
        Style::new().fg(Color::Black).bg(ACCENT).bold(),
    )];
    for state in [State::Conflict, State::Drifted, State::Snoozed] {
        let count = app
            .entries
            .iter()
            .filter(|entry| entry.state == state)
            .count();
        if count == 0 {
            continue;
        }
        let badge = state_badge(state);
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("{} {count} {}", badge.icon, badge.label),
            Style::new().fg(badge.color).bold(),
        ));
    }
    if app
        .entries
        .iter()
        .all(|entry| entry.state == State::Clean)
    {
        spans.push(Span::styled(
            "  ✓ all clean",
            Style::new().fg(Color::Green).bold(),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_sidebar(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let items = app.entries.iter().map(|entry| {
        let badge = state_badge(entry.state);
        ListItem::new(Line::from(vec![
            Span::styled(
                format!("{} ", badge.icon),
                Style::new().fg(badge.color).bold(),
            ),
            Span::raw(tilde(&entry.path)),
        ]))
    });
    let list = List::new(items)
        .block(pane_block(&format!("files · {}", app.entries.len())))
        .highlight_symbol("▌")
        .highlight_style(Style::new().bg(Color::DarkGray).bold());
    let mut state = ListState::default();
    state.select((!app.entries.is_empty()).then_some(app.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_detail(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let Some(entry) = app.entries.get(app.selected) else {
        let empty = Paragraph::new(Line::styled(
            "no managed files",
            Style::new().fg(DIM).bold(),
        ))
        .block(pane_block("diff"));
        frame.render_widget(empty, area);
        return;
    };
    // No wrapping: diff lines must keep their column alignment.
    let paragraph = Paragraph::new(Text::from(entry.detail.clone()))
        .block(pane_block(&tilde(&entry.path)))
        .scroll((app.scroll, 0));
    frame.render_widget(paragraph, area);
}

fn render_footer(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    if let Some(message) = &app.message {
        let line = Line::styled(format!(" {message}"), Style::new().fg(Color::Red).bold());
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    let mut spans = vec![Span::raw(" ")];
    for (keys, action) in [
        ("j/k", "files"),
        ("J/K", "scroll"),
        ("^d/^u", "page"),
        ("g/G", "ends"),
        ("⏎", "edit"),
        ("r", "refresh"),
        ("q", "quit"),
    ] {
        spans.push(Span::styled(keys, Style::new().fg(ACCENT).bold()));
        spans.push(Span::styled(format!(" {action}  "), Style::new().fg(DIM)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn pane_block(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM))
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(ACCENT).bold(),
        ))
}

fn tilde(path: &str) -> String {
    env::var("HOME")
        .ok()
        .and_then(|home| {
            let rest = path.strip_prefix(&home)?;
            rest.starts_with('/').then(|| format!("~{rest}"))
        })
        .unwrap_or_else(|| path.to_owned())
}

// --- detail content ---

fn detail_lines(entry: &TuiEntry) -> Vec<Line<'static>> {
    let badge = state_badge(entry.state);
    let persistence = match entry.persistence {
        Persistence::Ephemeral => "ephemeral · resets at next login",
        Persistence::Durable => "durable",
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("{} {}", badge.icon, badge.label),
            Style::new().fg(badge.color).bold(),
        ),
        Span::styled(format!("  {}", entry.format), Style::new().fg(ACCENT)),
        Span::styled(format!("  {persistence}"), Style::new().fg(DIM)),
    ])];
    if let Some(declared_at) = &entry.declared_at {
        lines.push(Line::styled(
            format!("declared at {declared_at}"),
            Style::new().fg(DIM),
        ));
    }
    lines.push(Line::default());
    if entry.state == State::Clean {
        lines.push(Line::styled(
            "✓ matches the declared base",
            Style::new().fg(Color::Green),
        ));
        return lines;
    }
    match &entry.incoming {
        Some(incoming) => {
            lines.push(section("your edits · base → file"));
            lines.extend(diff_lines(&entry.yours));
            lines.push(Line::default());
            lines.push(section("incoming · base → staged"));
            lines.extend(diff_lines(incoming));
            if !entry.overlap.is_empty() {
                lines.push(Line::default());
                lines.push(Line::styled(
                    "⚠ overlapping addresses",
                    Style::new().fg(Color::Red).bold(),
                ));
                for address in &entry.overlap {
                    lines.push(Line::styled(
                        format!("  {address}"),
                        Style::new().fg(Color::Red),
                    ));
                }
            }
        }
        None => lines.extend(diff_lines(&entry.yours)),
    }
    lines
}

fn diff_lines(diff: &TuiDiff) -> Vec<Line<'static>> {
    match diff {
        TuiDiff::Text { old, new } => unified(old, new),
        TuiDiff::Ops(ops) => ops_lines(ops),
    }
}

/// Logical ops as styled lines: the model diff for structured formats.
/// Values render for reading, not re-parsing — strings shed their quotes,
/// containers pretty-print — so one real edit reads as one line instead of
/// a raw-file hunk.
fn ops_lines(ops: &[Op]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for op in ops {
        match op {
            Op::Add { path, value } => entry_lines(&mut lines, '+', Color::Green, path, value),
            Op::Remove { path, from } => entry_lines(&mut lines, '-', Color::Red, path, from),
            Op::Replace { path, from, to } => {
                if let (Some(from), Some(to)) = (scalar(from), scalar(to)) {
                    lines.push(Line::from(vec![
                        Span::styled(format!("~ {path}  "), Style::new().fg(Color::Yellow)),
                        Span::styled(from, Style::new().fg(Color::Red)),
                        Span::styled(" → ", Style::new().fg(DIM)),
                        Span::styled(to, Style::new().fg(Color::Green)),
                    ]));
                } else {
                    lines.push(Line::styled(
                        format!("~ {path}"),
                        Style::new().fg(Color::Yellow),
                    ));
                    block_lines(&mut lines, '-', Color::Red, from);
                    block_lines(&mut lines, '+', Color::Green, to);
                }
            }
            Op::Text { diff } => lines.extend(diff.lines().map(patch_line)),
            Op::Binary => lines.push(Line::styled(
                "(binary contents differ)",
                Style::new().fg(DIM).italic(),
            )),
        }
    }
    if lines.is_empty() {
        lines.push(Line::styled(
            "(no logical changes; drift is formatting- or key-order-only)",
            Style::new().fg(DIM).italic(),
        ));
    }
    lines
}

/// `<mark> path  value` on one line for scalars, or `<mark> path` above the
/// container's pretty-printed block.
fn entry_lines(
    lines: &mut Vec<Line<'static>>,
    mark: char,
    color: Color,
    path: &str,
    value: &serde_json::Value,
) {
    if let Some(text) = scalar(value) {
        lines.push(Line::styled(
            format!("{mark} {path}  {text}"),
            Style::new().fg(color),
        ));
    } else {
        lines.push(Line::styled(
            format!("{mark} {path}"),
            Style::new().fg(color),
        ));
        block_lines(lines, mark, color, value);
    }
}

fn block_lines(
    lines: &mut Vec<Line<'static>>,
    mark: char,
    color: Color,
    value: &serde_json::Value,
) {
    for text in pretty(value) {
        lines.push(Line::styled(
            format!("{mark}   {text}"),
            Style::new().fg(color),
        ));
    }
}

/// A scalar rendered bare when its spelling is unambiguous; strings keep
/// their JSON quoting only where dropping it would lie (empty, multi-line,
/// or whitespace-trimmed). Containers return `None` and pretty-print.
fn scalar(value: &serde_json::Value) -> Option<String> {
    use serde_json::Value;
    match value {
        Value::String(text) => {
            let ambiguous = text.is_empty() || text.contains('\n') || text.trim() != text;
            Some(if ambiguous {
                value.to_string()
            } else {
                text.clone()
            })
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => Some(value.to_string()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

fn pretty(value: &serde_json::Value) -> Vec<String> {
    serde_json::to_string_pretty(value)
        .expect("a Value has no non-string keys, so pretty-printing cannot fail")
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Style one line of an already-rendered unified diff by its marker.
fn patch_line(text: &str) -> Line<'static> {
    let style = match text.as_bytes().first() {
        Some(b'@') => Style::new().fg(ACCENT),
        Some(b'-') => Style::new().fg(Color::Red),
        Some(b'+') => Style::new().fg(Color::Green),
        _ => Style::new().fg(DIM),
    };
    Line::styled(text.to_owned(), style)
}

fn section(title: &str) -> Line<'static> {
    Line::styled(
        format!("── {title} ──"),
        Style::new().fg(Color::Magenta).bold(),
    )
}

/// A unified diff as styled lines: cyan hunk headers, red deletions, green
/// insertions, dim context.
fn unified(old: &str, new: &str) -> Vec<Line<'static>> {
    let diff = TextDiff::from_lines(old, new);
    let mut lines = Vec::new();
    for group in diff.grouped_ops(3) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        lines.push(Line::styled(
            format!(
                "@@ -{},{} +{},{} @@",
                first.old_range().start + 1,
                last.old_range().end - first.old_range().start,
                first.new_range().start + 1,
                last.new_range().end - first.new_range().start,
            ),
            Style::new().fg(ACCENT),
        ));
        for op in &group {
            for change in diff.iter_changes(op) {
                let text = change.value().trim_end_matches('\n');
                lines.push(match change.tag() {
                    ChangeTag::Delete => {
                        Line::styled(format!("-{text}"), Style::new().fg(Color::Red))
                    }
                    ChangeTag::Insert => {
                        Line::styled(format!("+{text}"), Style::new().fg(Color::Green))
                    }
                    ChangeTag::Equal => Line::styled(format!(" {text}"), Style::new().fg(DIM)),
                });
            }
        }
    }
    if lines.is_empty() {
        lines.push(Line::styled(
            "(no line-level changes; drift is formatting- or key-order-only)",
            Style::new().fg(DIM).italic(),
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Format;

    fn text_diff(old: &str, new: &str) -> TuiDiff {
        TuiDiff::Text {
            old: old.to_owned(),
            new: new.to_owned(),
        }
    }

    fn entry(state: State, staged: Option<&str>, overlap: Vec<String>) -> TuiEntry {
        let base = "{\n  \"a\": 1\n}\n";
        TuiEntry {
            path: "/tmp/config.json".to_owned(),
            state,
            format: Format::Json,
            persistence: Persistence::Durable,
            declared_at: Some("home/test.nix:1".to_owned()),
            yours: text_diff(base, "{\n  \"a\": 2\n}\n"),
            incoming: staged.map(|staged| text_diff(base, staged)),
            overlap,
        }
    }

    fn rendered(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
    }

    fn app() -> App {
        let entries = [entry(State::Drifted, None, Vec::new()), {
            let mut second = entry(State::Conflict, None, Vec::new());
            second.path = "/tmp/other.json".to_owned();
            second
        }]
        .iter()
        .map(|entry| Entry {
            path: entry.path.clone(),
            state: entry.state,
            detail: detail_lines(entry),
        })
        .collect();
        App {
            entries,
            selected: 0,
            scroll: 0,
            message: None,
        }
    }

    #[test]
    fn unified_marks_deletions_and_insertions() {
        let lines = rendered(&unified("a\nb\n", "a\nc\n"));
        assert!(lines[0].starts_with("@@ "));
        assert!(lines.contains(&"-b".to_owned()));
        assert!(lines.contains(&"+c".to_owned()));
    }

    #[test]
    fn unified_reports_formatting_only_drift() {
        let lines = rendered(&unified("same\n", "same\n"));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("formatting"));
    }

    #[test]
    fn clean_detail_reports_in_sync_without_a_diff() {
        let entry = entry(State::Clean, None, Vec::new());
        let text = rendered(&detail_lines(&entry)).join("\n");
        assert!(text.contains("✓ matches the declared base"));
        assert!(!text.contains("@@"), "clean entries must not render a diff");
    }

    #[test]
    fn conflict_detail_shows_both_sides_and_overlap() {
        let entry = entry(
            State::Conflict,
            Some("{\n  \"a\": 3\n}\n"),
            vec!["a".to_owned()],
        );
        let text = rendered(&detail_lines(&entry)).join("\n");
        assert!(text.contains("your edits · base → file"));
        assert!(text.contains("incoming · base → staged"));
        assert!(text.contains("⚠ overlapping addresses"));
    }

    #[test]
    fn navigation_clamps_and_resets_scroll() {
        let mut app = app();
        app.select(app.selected.saturating_sub(1));
        assert_eq!(app.selected, 0);
        app.select(usize::MAX);
        assert_eq!(app.selected, 1);
        app.scroll_down(200);
        assert_eq!(app.scroll, app.max_scroll());
        app.select(0);
        assert_eq!(app.scroll, 0);
        app.scroll_down(2);
        app.scroll_up(200);
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn ops_render_addressed_edits_and_binary_fallback() {
        let ops = vec![
            Op::Add {
                path: "/a".to_owned(),
                value: 1.into(),
            },
            Op::Remove {
                path: "/b".to_owned(),
                from: 2.into(),
            },
            Op::Replace {
                path: "/c".to_owned(),
                from: 3.into(),
                to: 4.into(),
            },
            Op::Binary,
        ];
        let lines = rendered(&ops_lines(&ops));
        assert_eq!(lines[0], "+ /a  1");
        assert_eq!(lines[1], "- /b  2");
        assert_eq!(lines[2], "~ /c  3 → 4");
        assert_eq!(lines[3], "(binary contents differ)");
        assert_eq!(
            rendered(&ops_lines(&[]))[0],
            "(no logical changes; drift is formatting- or key-order-only)"
        );
    }

    #[test]
    fn ops_render_strings_bare_and_containers_pretty() {
        use serde_json::json;
        let ops = vec![
            Op::Add {
                path: "/model".to_owned(),
                value: json!("claude-fable-5[1m]"),
            },
            Op::Replace {
                path: "/mode".to_owned(),
                from: json!("auto"),
                to: json!(" padded"),
            },
            Op::Add {
                path: "/rules".to_owned(),
                value: json!({"deep": true}),
            },
        ];
        let lines = rendered(&ops_lines(&ops));
        assert_eq!(lines[0], "+ /model  claude-fable-5[1m]");
        // Ambiguous strings keep their quotes.
        assert_eq!(lines[1], "~ /mode  auto → \" padded\"");
        assert_eq!(lines[2], "+ /rules");
        assert_eq!(lines[3], "+   {");
        assert_eq!(lines[4], "+     \"deep\": true");
        assert_eq!(lines[5], "+   }");
    }

    #[test]
    fn tilde_shortens_only_whole_home_prefix() {
        let home = std::env::var("HOME").expect("HOME");
        assert_eq!(tilde(&format!("{home}/x.json")), "~/x.json");
        assert_eq!(
            tilde(&format!("{home}stead/x.json")),
            format!("{home}stead/x.json")
        );
    }
}
