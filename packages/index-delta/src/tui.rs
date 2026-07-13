use std::io::{self, Stdout};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::{cmd::tui_entries, store::Store};

type TuiTerminal = Terminal<CrosstermBackend<Stdout>>;

struct App {
    entries: Vec<Entry>,
    selected: usize,
}

struct Entry {
    path: String,
    state: String,
    diff: String,
}

impl App {
    const fn new(entries: Vec<Entry>) -> Self {
        Self {
            entries,
            selected: 0,
        }
    }

    fn move_down(&mut self) {
        if !self.entries.is_empty() {
            self.selected = (self.selected + 1).min(self.entries.len() - 1);
        }
    }

    const fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    const fn move_first(&mut self) {
        self.selected = 0;
    }

    const fn move_last(&mut self) {
        self.selected = self.entries.len().saturating_sub(1);
    }
}

pub fn run(store: &Store) -> Result<()> {
    let entries = tui_entries(store)?
        .into_iter()
        .map(|entry| Entry {
            path: entry.path,
            state: entry.state,
            diff: entry.diff,
        })
        .collect();
    let mut terminal = init_terminal()?;
    let result = run_loop(&mut terminal, App::new(entries));
    restore_terminal(&mut terminal)?;
    result
}

fn init_terminal() -> Result<TuiTerminal> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stdout)).map_err(Into::into)
}

fn restore_terminal(terminal: &mut TuiTerminal) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn run_loop(terminal: &mut TuiTerminal, mut app: App) -> Result<()> {
    loop {
        terminal.draw(|frame| render(frame, &app))?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Char('j') | KeyCode::Down => app.move_down(),
            KeyCode::Char('k') | KeyCode::Up => app.move_up(),
            KeyCode::Char('g') | KeyCode::Home => app.move_first(),
            KeyCode::Char('G') | KeyCode::End => app.move_last(),
            _ => {}
        }
    }
}

fn render(frame: &mut ratatui::Frame, app: &App) {
    let [body, footer] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
    let [sidebar, detail_area] =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)]).areas(body);
    let items = app
        .entries
        .iter()
        .map(|entry| ListItem::new(Line::from(format!("{} {}", entry.state, entry.path))));
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Mutable files"),
        )
        .highlight_style(Style::default().bg(Color::DarkGray));
    let mut state = ListState::default();
    state.select((!app.entries.is_empty()).then_some(app.selected));
    frame.render_stateful_widget(list, sidebar, &mut state);

    let detail_text = app
        .entries
        .get(app.selected)
        .map_or("No mutable-file drift to review.", |entry| {
            entry.diff.as_str()
        });
    let detail = Paragraph::new(detail_text)
        .block(Block::default().borders(Borders::ALL).title("Diff"))
        .wrap(Wrap { trim: false });
    frame.render_widget(detail, detail_area);
    let help = Line::from("j/k move  g/G first/last  q quit");
    frame.render_widget(Paragraph::new(help), footer);
}

#[cfg(test)]
mod tests {
    use super::{App, Entry};

    fn app() -> App {
        App::new(vec![
            Entry {
                path: "a".into(),
                state: "drifted".into(),
                diff: String::new(),
            },
            Entry {
                path: "b".into(),
                state: "conflict".into(),
                diff: String::new(),
            },
        ])
    }

    #[test]
    fn vim_navigation_stays_within_entries() {
        let mut app = app();
        app.move_up();
        assert_eq!(app.selected, 0);
        app.move_last();
        app.move_down();
        assert_eq!(app.selected, 1);
        app.move_first();
        assert_eq!(app.selected, 0);
    }
}
