//! Rendering: the fleet list, the attached session, and the shortcut sheet.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ndarray::Array2;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Widget};
use tui::{Color as VtColor, StyledCell};
use uuid::Uuid;

use crate::app::{App, Entry, Mode};
use crate::session::Status;

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;
/// Braille spinner, one frame per repaint tick.
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub fn draw(frame: &mut Frame, app: &App, tick: usize) {
    match app.mode {
        Mode::List => list(frame, app, tick),
        Mode::Attached(id) => attached(frame, app, id),
        Mode::Help => {
            list(frame, app, tick);
            help(frame);
        }
    }
}

fn list(frame: &mut Frame, app: &App, tick: usize) {
    let area = frame.area();
    let [header, body, prompt] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .areas(area);

    frame.render_widget(header_lines(app), header);
    rows(frame, app, body, tick);
    frame.render_widget(prompt_lines(app), prompt);
}

fn header_lines(app: &App) -> Paragraph<'_> {
    let [awaiting, working, completed] = app.counts();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let where_ = format!(
        "{} · {}",
        short_path(&app.defaults.cwd, home.as_deref()),
        app.defaults.command
    );
    Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                "fleetview",
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {where_}"), Style::new().fg(DIM)),
        ]),
        Line::from(vec![
            count_span(awaiting, "awaiting input", Color::Green),
            Span::styled(" · ", Style::new().fg(DIM)),
            count_span(working, "working", Color::Yellow),
            Span::styled(" · ", Style::new().fg(DIM)),
            count_span(completed, "completed", DIM),
        ]),
        Line::from(app.message.as_deref().map_or_else(
            || Span::raw(""),
            |message| Span::styled(message, Style::new().fg(Color::Red)),
        )),
    ])
}

/// A count is only worth colour when it is non-zero; a fleet of zeros should
/// read as one grey line rather than a christmas tree.
fn count_span(count: usize, label: &str, color: Color) -> Span<'static> {
    let style = if count == 0 {
        Style::new().fg(DIM)
    } else {
        Style::new().fg(color)
    };
    Span::styled(format!("{count} {label}"), style)
}

fn rows(frame: &mut Frame, app: &App, area: Rect, tick: usize) {
    let entries = app.view();
    if entries.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "  no sessions yet — type a task below and press enter",
                Style::new().fg(DIM),
            )),
            area,
        );
        return;
    }

    let mut items: Vec<ListItem<'_>> = Vec::new();
    let mut selected_row = None;
    let mut section = None;
    for entry in &entries {
        if section != Some(entry.snapshot.status) {
            section = Some(entry.snapshot.status);
            items.push(ListItem::new(Line::styled(
                format!(" {}", entry.snapshot.status.heading()),
                Style::new().fg(DIM).add_modifier(Modifier::BOLD),
            )));
        }
        if app.selected() == Some(entry.session.id) {
            selected_row = Some(items.len());
        }
        items.push(ListItem::new(row(entry, area.width, tick)));
    }

    let list = List::new(items).highlight_style(Style::new().bg(Color::Rgb(38, 44, 52)));
    let mut state = ListState::default();
    state.select(selected_row);
    frame.render_stateful_widget(list, area, &mut state);
}

/// One session: glyph, task, whatever its screen last said, and its age.
fn row<'a>(entry: &'a Entry<'a>, width: u16, tick: usize) -> Line<'a> {
    let (glyph, color) = badge(entry, tick);
    let age = elapsed(entry.session.elapsed());
    // Three columns out of the width left after the glyph and the age.
    let content = usize::from(width).saturating_sub(4 + age.len() + 2);
    let title_width = (content * 2 / 5).max(8);
    let preview_width = content.saturating_sub(title_width + 2);

    Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("{glyph} "), Style::new().fg(color)),
        Span::raw(fit(&entry.session.label(), title_width)),
        Span::raw("  "),
        Span::styled(
            fit(&entry.snapshot.preview, preview_width),
            Style::new().fg(DIM),
        ),
        Span::styled(format!("  {age}"), Style::new().fg(DIM)),
    ])
}

fn badge(entry: &Entry<'_>, tick: usize) -> (char, Color) {
    match entry.snapshot.status {
        Status::Working => (
            SPINNER.get(tick % SPINNER.len()).copied().unwrap_or('*'),
            Color::Yellow,
        ),
        // A blocked question and a finished turn both want a human, but only
        // one of them is holding the agent still, so they are not one glyph.
        Status::AwaitingInput if entry.snapshot.gate => ('◆', Color::Magenta),
        Status::AwaitingInput => ('●', Color::Green),
        Status::Completed => match entry.snapshot.exit_code {
            Some(0) | None => ('✓', DIM),
            Some(_) => ('✗', Color::Red),
        },
    }
}

fn prompt_lines(app: &App) -> Paragraph<'_> {
    let typed = if app.input.is_empty() {
        Span::styled("describe a task for a new session", Style::new().fg(DIM))
    } else {
        Span::raw(app.input.as_str())
    };
    Paragraph::new(vec![
        Line::from(vec![
            Span::styled(" › ", Style::new().fg(ACCENT)),
            typed,
            Span::styled("▌", Style::new().fg(ACCENT)),
        ]),
        Line::styled(
            "   enter to dispatch or open · ctrl-n/ctrl-p to move · ? for shortcuts",
            Style::new().fg(DIM),
        ),
    ])
}

/// The attached session: its own screen, cell for cell, over a one-line footer.
fn attached(frame: &mut Frame, app: &App, id: Uuid) {
    let area = frame.area();
    let [screen, footer] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    let Some(session) = app.session(id) else {
        frame.render_widget(
            Paragraph::new(Line::styled("session is gone", Style::new().fg(DIM))),
            area,
        );
        return;
    };

    if let Ok(cells) = session.instance.read_styled_cells() {
        frame.render_widget(Screen { cells: &cells }, screen);
    }
    if let Ok(cursor) = session.instance.read_cursor()
        && cursor.visible
        && cursor.row < screen.height
        && cursor.col < screen.width
    {
        frame.set_cursor_position((screen.x + cursor.col, screen.y + cursor.row));
    }

    let snapshot = session.snapshot();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ctrl-o ", Style::new().fg(Color::Black).bg(ACCENT)),
            Span::styled(" detach  ", Style::new().fg(DIM)),
            Span::raw(fit(session.title(), 40)),
            Span::styled(
                format!("  {}", snapshot.status.heading()),
                Style::new().fg(DIM),
            ),
        ])),
        footer,
    );
}

/// The VT grid painted straight into the ratatui buffer.
struct Screen<'a> {
    cells: &'a Array2<StyledCell>,
}

impl Widget for Screen<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        for (position, cell) in self.cells.indexed_iter() {
            let (Ok(row), Ok(col)) = (u16::try_from(position.0), u16::try_from(position.1)) else {
                continue;
            };
            if row >= area.height || col >= area.width {
                continue;
            }
            let Some(target) = buf.cell_mut((area.x + col, area.y + row)) else {
                continue;
            };
            target.set_char(cell.character);
            target.set_style(cell_style(cell));
        }
    }
}

fn cell_style(cell: &StyledCell) -> Style {
    let mut style = Style::new().fg(color(cell.fg)).bg(color(cell.bg));
    if cell.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse {
        style = style.add_modifier(Modifier::REVERSED);
    }
    style
}

const fn color(color: VtColor) -> Color {
    match color {
        // The terminal's own default, not a guess at what it might be.
        VtColor::Default => Color::Reset,
        VtColor::Indexed(index) => Color::Indexed(index),
        VtColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn help(frame: &mut Frame) {
    let keys = [
        (
            "type + enter",
            "dispatch a new session in the working directory",
        ),
        ("+agent task", "dispatch under `claude --agent agent`"),
        ("enter (empty)", "attach to the selected session"),
        ("ctrl-n / ctrl-p", "move the selection (arrows work too)"),
        ("ctrl-o", "detach from a session, back to the list"),
        ("ctrl-w / ctrl-u", "erase a word / the whole prompt"),
        ("ctrl-x", "stop the selected agent"),
        ("ctrl-d", "dismiss a finished session"),
        ("ctrl-c", "quit, stopping every agent"),
    ];
    let lines: Vec<Line<'_>> = keys
        .into_iter()
        .map(|(key, what)| {
            Line::from(vec![
                Span::styled(format!(" {key:<16}"), Style::new().fg(ACCENT)),
                Span::raw(what),
            ])
        })
        .collect();

    let area = centered(frame.area(), 68, 11);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(DIM))
                .title(" shortcuts "),
        ),
        area,
    );
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// Truncate to `width` columns, marking the cut so a clipped task does not read
/// as a complete one.
fn fit(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return format!("{text:<width$}");
    }
    let keep = width.saturating_sub(1);
    let mut out: String = text.chars().take(keep).collect();
    out.push('…');
    out
}

/// Where the fleet is working, short enough to leave room for the rest of the
/// header: `~` for home, and only the last two components once it is still long.
fn short_path(path: &Path, home: Option<&Path>) -> String {
    let full = home
        .and_then(|home| path.strip_prefix(home).ok())
        .map_or_else(
            || path.display().to_string(),
            |rest| format!("~/{}", rest.display()),
        );
    if full.chars().count() <= 30 {
        return full;
    }
    let mut tail = full.rsplit('/');
    let (Some(last), Some(parent)) = (tail.next(), tail.next()) else {
        return full;
    };
    let root = if full.starts_with('~') { "~" } else { "" };
    format!("{root}/…/{parent}/{last}")
}

/// Age at a glance: seconds, then minutes, then hours.
fn elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        _ => format!("{}h", seconds / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::{elapsed, fit, short_path};
    use std::path::Path;
    use std::time::Duration;

    #[test]
    fn a_short_path_under_home_is_just_tilded() {
        assert_eq!(
            short_path(Path::new("/home/me/src/index"), Some(Path::new("/home/me"))),
            "~/src/index"
        );
    }

    #[test]
    fn a_long_path_keeps_only_the_last_two_components() {
        assert_eq!(
            short_path(
                Path::new("/home/me/.config/nix/ix/index/.claude/worktrees/fleetview"),
                Some(Path::new("/home/me"))
            ),
            "~/…/worktrees/fleetview"
        );
    }

    #[test]
    fn a_path_outside_home_shortens_without_a_tilde() {
        assert_eq!(
            short_path(
                Path::new("/var/lib/some/very/long/path/to/a/checkout"),
                Some(Path::new("/home/me"))
            ),
            "/…/a/checkout"
        );
    }

    #[test]
    fn a_path_with_no_home_to_strip_still_renders() {
        assert_eq!(short_path(Path::new("/srv/repo"), None), "/srv/repo");
    }

    #[test]
    fn short_text_is_padded_to_the_column_width() {
        assert_eq!(fit("ok", 5), "ok   ");
    }

    #[test]
    fn long_text_is_cut_with_an_ellipsis() {
        assert_eq!(fit("abcdefgh", 4), "abc…");
    }

    #[test]
    fn truncation_counts_characters_not_bytes() {
        assert_eq!(fit("héllo wörld", 6), "héllo…");
    }

    #[test]
    fn age_steps_from_seconds_to_minutes_to_hours() {
        assert_eq!(elapsed(Duration::from_secs(53)), "53s");
        assert_eq!(elapsed(Duration::from_secs(60)), "1m");
        assert_eq!(elapsed(Duration::from_secs(32 * 60)), "32m");
        assert_eq!(elapsed(Duration::from_secs(2 * 3600 + 61)), "2h");
    }
}
