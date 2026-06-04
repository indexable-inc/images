//! Sample a live [`TuiManager`](crate::TuiManager) into the engine-free wire
//! panes shared by the producer ([`crate::publish`]) and the in-process
//! dashboard ([`crate::dashboard`]).
//!
//! The wire shapes themselves ([`Pane`], [`TerminalView`], [`ProducerSnapshot`])
//! and the discovery paths live in `dashboard-core` so the aggregator can render
//! them without the PTY engine; `tui` re-exports them. This module owns only the
//! bridge that reads a terminal out of a manager and wraps it as a pane, which is
//! the one half that needs the engine.

mod sgr;

use dashboard_core::{Pane, TerminalView};

/// Sample every terminal the manager tracks into a pane list.
///
/// A terminal whose styled-cell read fails this tick is skipped, not dropped
/// from the set: the next tick re-reads it. The screen is encoded as minimal
/// ANSI SGR ([`sgr::encode`]) so the dashboard can paint color and attributes;
/// the cursor position and shape and the exit code ride alongside. Shared by the
/// in-process dashboard poller and the producer so both render the same snapshot
/// of a manager.
pub async fn collect_panes(manager: &crate::TuiManager) -> Vec<Pane> {
    let mut panes = Vec::new();
    for instance in manager.list() {
        let Ok(cells) = instance.read_styled_cells_async().await else {
            continue;
        };
        // Cursor and exit are best-effort: a failed cursor read defaults to the
        // top-left, never dropping the whole pane.
        let cursor = instance.read_cursor_async().await.unwrap_or(crate::CursorPos {
            row: 0,
            col: 0,
            visible: true,
        });
        let rows: Vec<Vec<crate::StyledCell>> =
            cells.rows().into_iter().map(|row| row.to_vec()).collect();

        panes.push(Pane::terminal(
            instance.id.to_string(),
            TerminalView {
                command: instance.command.clone(),
                args: instance.args.join(" "),
                rows: instance.rows(),
                cols: instance.cols(),
                alive: instance.is_alive(),
                screen: sgr::encode(&rows),
                cursor_row: cursor.row,
                cursor_col: cursor.col,
                cursor_visible: cursor.visible,
                cursor_shape: instance.cursor_shape().as_str().to_owned(),
                exit_code: match instance.exit_state() {
                    crate::ExitState::Exited(code) => code,
                    crate::ExitState::Running => None,
                },
            },
        ));
    }
    panes
}
