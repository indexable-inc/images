//! Drive a real shell session through the [`tui`] PTY driver and collect frames.
//!
//! A `bash` is spawned on a PTY with a clean prompt, the script is typed into
//! it, and the VT-rendered grid is sampled once per frame interval. Sampling on
//! a wall-clock cadence lets the child's real output stream into the capture,
//! so the recording shows the actual program running, not a faked transcript.

use std::thread::sleep;
use std::time::Duration;

use color_eyre::eyre::{Result, WrapErr};
use tui::{SpawnConfig, TuiInstance, TuiManager};

use crate::scene::{Action, Cursor, Frame, PROMPT, script};

/// Record the demo script into a sequence of terminal frames.
pub fn record(cols: u16, rows: u16, fps: u32) -> Result<Vec<Frame>> {
    let frame_dur = Duration::from_secs_f32(1.0 / fps as f32);
    let manager = TuiManager::new();
    let term = manager
        .spawn(
            "bash".to_owned(),
            vec![
                "--noprofile".to_owned(),
                "--norc".to_owned(),
                "-i".to_owned(),
            ],
            SpawnConfig {
                rows,
                cols,
                scrollback_lines: 2000,
            },
        )
        .wrap_err("spawn bash")?;

    // Hidden setup, not captured: a clean prompt, no history file, a known TERM,
    // and a cleared screen.
    term.write(&format!(
        "export PS1='{PROMPT}' PS2='' TERM=xterm-256color HISTFILE=/dev/null\n"
    ))
    .wrap_err("set prompt")?;
    sleep(Duration::from_millis(250));
    term.write("clear\n").wrap_err("clear screen")?;
    sleep(Duration::from_millis(450));

    let mut frames = Vec::new();
    for action in script(fps) {
        run_action(&term, &action, frame_dur, &mut frames)?;
    }
    Ok(frames)
}

/// Capture one frame: the styled grid plus the cursor.
fn capture(term: &TuiInstance, frames: &mut Vec<Frame>) -> Result<()> {
    let cells = term.read_styled_cells().wrap_err("read styled cells")?;
    let (row, col, visible) = term.read_cursor().wrap_err("read cursor")?;
    frames.push(Frame::Terminal {
        cells,
        cursor: Cursor { row, col, visible },
    });
    Ok(())
}

/// Run one action, capturing frames as it progresses.
fn run_action(
    term: &TuiInstance,
    action: &Action,
    frame_dur: Duration,
    frames: &mut Vec<Frame>,
) -> Result<()> {
    match action {
        Action::Type(text) => {
            // Emit one frame per few characters rather than per character: the
            // typing still reads as live, but the frame count (and so the WebP
            // size) stays bounded for long commands.
            const CHARS_PER_FRAME: usize = 2;
            let chars: Vec<char> = text.chars().collect();
            for chunk in chars.chunks(CHARS_PER_FRAME) {
                let mut piece = String::new();
                piece.extend(chunk);
                term.write(&piece).wrap_err("type")?;
                sleep(frame_dur);
                capture(term, frames)?;
            }
        }
        Action::Send(raw) => {
            term.write(raw).wrap_err("send")?;
            sleep(frame_dur);
            capture(term, frames)?;
        }
        Action::Hold(count) => {
            for _ in 0..*count {
                sleep(frame_dur);
                capture(term, frames)?;
            }
        }
        Action::WaitFor { needle, max } => {
            for _ in 0..*max {
                sleep(frame_dur);
                capture(term, frames)?;
                let viewport = term.read_viewport().wrap_err("read viewport")?;
                if viewport.iter().any(|line| line.contains(*needle)) {
                    break;
                }
            }
        }
    }
    Ok(())
}
