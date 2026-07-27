//! `fleetview`: dispatch Claude Code sessions and watch the whole fleet at once.
//!
//! Every session is a real `claude` process on its own PTY, rendered by
//! ghostty's VT engine through the repo's `tui` crate. The list infers what
//! each one is doing from screen activity alone — there is no "turn finished"
//! event to subscribe to — and `enter` attaches to one full-screen, keys and
//! all, so the agent cannot tell it is not in your terminal.
//!
//! ```text
//! fleetview  ~/src/index · claude
//! 1 awaiting input · 2 working · 0 completed
//!
//!  awaiting input
//!    ● fix the flake eval      3 files changed, all green      32m
//!  working
//!    ⠹ ship the PR             Bash(cargo test --workspace)    53s
//!
//!  › describe a task for a new session
//! ```

mod app;
mod keys;
mod session;
mod ui;

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use color_eyre::Result;
use color_eyre::eyre::WrapErr;
use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use app::{App, Defaults, Flow};

/// Repaint cadence when no key arrives. Fast enough for the spinner to read as
/// motion, slow enough that a screenful of idle sessions costs nothing.
const TICK: Duration = Duration::from_millis(100);

type Term = Terminal<CrosstermBackend<Stdout>>;

#[derive(Parser, Debug)]
#[command(
    name = "fleetview",
    version,
    about = "Watch and drive a fleet of Claude Code sessions"
)]
struct Cli {
    /// Directory the dispatched agents run in. Defaults to the current one.
    #[arg(long, value_name = "DIR")]
    cwd: Option<PathBuf>,

    /// The agent binary to spawn. Anything that reads a prompt argument and
    /// paints a TUI works: `claude`, `codex`, `cursor-agent`.
    #[arg(long, default_value = "claude", value_name = "BIN")]
    command: String,

    /// Default subagent for dispatches that do not name one with `+agent`.
    #[arg(long, value_name = "NAME")]
    agent: Option<String>,

    /// Model for every dispatched session.
    #[arg(long, value_name = "NAME")]
    model: Option<String>,

    /// Extra arguments appended to every dispatch, after `--`.
    #[arg(last = true, value_name = "ARG")]
    extra: Vec<String>,
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    let cwd = match cli.cwd {
        Some(dir) => dir,
        None => std::env::current_dir().wrap_err("no working directory")?,
    };
    let defaults = Defaults {
        command: cli.command,
        cwd,
        agent: cli.agent,
        model: cli.model,
        extra: cli.extra,
    };

    let (cols, rows) = crossterm::terminal::size().wrap_err("terminal has no size")?;
    let mut app = App::new(defaults, rows, cols);

    let mut terminal = enter().wrap_err("could not take over the terminal")?;
    let outcome = run(&mut terminal, &mut app);
    // Restore the terminal whatever happened, then report: a raw-mode terminal
    // left behind is worse than the error that caused it.
    app.shutdown();
    leave(&mut terminal)?;
    outcome
}

fn enter() -> Result<Term> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn leave(terminal: &mut Term) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn run(terminal: &mut Term, app: &mut App) -> Result<()> {
    let mut tick = 0_usize;
    loop {
        terminal.draw(|frame| ui::draw(frame, app, tick))?;
        if event::poll(TICK)? {
            match event::read()? {
                // Release events arrive on terminals with the kitty keyboard
                // protocol; forwarding them would double every keystroke.
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    if app.on_key(key) == Flow::Quit {
                        return Ok(());
                    }
                }
                Event::Paste(text) => app.on_paste(&text),
                Event::Resize(cols, rows) => app.on_resize(rows, cols),
                _ => {}
            }
        }
        tick = tick.wrapping_add(1);
    }
}
