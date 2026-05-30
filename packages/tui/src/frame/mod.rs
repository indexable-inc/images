//! Sample a live [`TuiManager`](crate::TuiManager) into the engine-free wire
//! frames shared by the producer ([`crate::publish`]) and the in-process
//! dashboard ([`crate::dashboard`]).
//!
//! The wire shapes themselves ([`TerminalFrame`], [`ProducerSnapshot`]) and the
//! discovery paths live in `tui-dashboard-core` so the aggregator can render
//! them without the PTY engine; `tui` re-exports them. This module owns only the
//! bridge that reads a frame out of a manager, which is the one half that needs
//! the engine.

mod sgr;

use tui_dashboard_core::TerminalFrame;

/// Sample every terminal the manager tracks into a frame list.
///
/// A terminal whose styled-cell read fails this tick is skipped, not dropped
/// from the set: the next tick re-reads it. The screen is encoded as minimal
/// ANSI SGR ([`sgr::encode`]) so the dashboard can paint color and attributes;
/// the cursor position and shape and the exit code ride alongside. Shared by the
/// in-process dashboard poller and the producer so both render the same
/// snapshot of a manager.
pub async fn collect_frames(manager: &crate::TuiManager) -> Vec<TerminalFrame> {
    let mut frames = Vec::new();
    for instance in manager.list() {
        let Ok(cells) = instance.read_styled_cells_async().await else {
            continue;
        };
        // Cursor and exit are best-effort: a failed cursor read defaults to the
        // top-left, never dropping the whole frame.
        let (cursor_row, cursor_col, cursor_visible) =
            instance.read_cursor_async().await.unwrap_or((0, 0, true));
        let rows: Vec<Vec<crate::StyledCell>> =
            cells.rows().into_iter().map(|row| row.to_vec()).collect();

        frames.push(TerminalFrame {
            id: instance.id.to_string(),
            command: instance.command.clone(),
            args: instance.args.join(" "),
            rows: instance.rows(),
            cols: instance.cols(),
            alive: instance.is_alive(),
            screen: sgr::encode(&rows),
            cursor_row,
            cursor_col,
            cursor_visible,
            cursor_shape: instance.cursor_shape().as_str().to_owned(),
            exit_code: match instance.exit_state() {
                crate::ExitState::Exited(code) => code,
                crate::ExitState::Running => None,
            },
        });
    }
    frames
}
