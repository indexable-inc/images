//! Interactive ratatui front end: a vim-flavored table over the patch rows.
//!
//! Keys: `j`/`k` (or arrows) move, `gg`/`G` jump, `/` filters, `Enter` opens
//! the selected patch's PR in the browser, `Esc` clears the filter, `q` quits.

use std::process::{Command, Stdio};

use color_eyre::eyre::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};

use crate::model::{CiState, PatchRow, PrSource, PrState, ReviewState};
use crate::plain;

pub fn run(rows: Vec<PatchRow>, note: Option<String>) -> Result<()> {
    let mut terminal = ratatui::init();
    let mut app = App::new(rows, note);
    let result = loop {
        if let Err(error) = terminal.draw(|frame| app.draw(frame)) {
            break Err(error.into());
        }
        match event::read() {
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                if app.handle_key(key) {
                    break Ok(());
                }
            }
            Ok(_) => {}
            Err(error) => break Err(error.into()),
        }
    };
    ratatui::restore();
    result
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
}

impl App {
    fn new(rows: Vec<PatchRow>, note: Option<String>) -> Self {
        let mut app = Self {
            visible: (0..rows.len()).collect(),
            rows,
            table: TableState::default(),
            filter: String::new(),
            filter_active: false,
            pending_g: false,
            message: note.unwrap_or_default(),
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

    /// Returns true when the app should quit.
    fn handle_key(&mut self, key: KeyEvent) -> bool {
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
            return false;
        }
        let pending_g = std::mem::take(&mut self.pending_g);
        match key.code {
            KeyCode::Char('q') => return true,
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
            _ => {}
        }
        false
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [table_area, footer_area] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
        self.draw_table(frame, table_area);
        self.draw_footer(frame, footer_area);
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

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let line = if self.filter_active {
            Line::from(vec![
                Span::styled("/", Style::new().fg(Color::Yellow)),
                Span::raw(self.filter.clone()),
                Span::styled("_", Style::new().add_modifier(Modifier::SLOW_BLINK)),
            ])
        } else if self.message.is_empty() {
            let mut spans = vec![Span::styled(
                " j/k move  gg/G jump  / filter  Enter open PR  q quit ",
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
