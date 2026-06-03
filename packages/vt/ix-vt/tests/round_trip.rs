//! Round-trip smoke test mirroring the C `proof.c` from the spike.
//!
//! Writes SGR (bold + palette red, underline) plus DECSCUSR and a cursor move,
//! then snapshots the render state and asserts the cursor shape and position,
//! and a styled cell's foreground and flags. This proves the full
//! `vt_write -> render -> read cell/cursor` path through the safe wrapper.

use ix_vt::{CursorVisualStyle, StyleColor, Terminal};

#[test]
fn sgr_cursor_and_cell_round_trip() {
    let mut term = Terminal::new(24, 80, 1000).expect("create terminal");

    // ESC[1;31m "RED" ESC[0m " " ESC[4m "under" ESC[0m
    term.vt_write(b"\x1b[1;31mRED\x1b[0m \x1b[4munder\x1b[0m");
    // DECSCUSR bar cursor: ESC[6 q (note the space before q).
    term.vt_write(b"\x1b[6 q");
    // Move cursor to row 5, col 10 (1-indexed CUP): ESC[5;10H.
    term.vt_write(b"\x1b[5;10H");

    let snap = term.render().expect("render snapshot");

    // Cursor: bar shape, viewport position (col 9, row 4) zero-indexed.
    assert_eq!(snap.cursor.visual_style, CursorVisualStyle::Bar, "cursor is a bar");
    assert!(snap.cursor.visible, "cursor visible");
    assert_eq!(
        snap.cursor.viewport,
        Some((9, 4)),
        "cursor at col 9 row 4 (ESC[5;10H, zero-indexed)"
    );

    // Cell at row 0 col 0: 'R' from ESC[1;31m -> bold + palette red (index 1).
    let cell0 = &snap.viewport[0][0];
    assert_eq!(cell0.ch, Some('R'), "first cell is 'R'");
    assert!(cell0.style.bold, "first cell is bold");
    assert_eq!(
        cell0.style.fg_color,
        StyleColor::Palette(1),
        "first cell fg is palette red (index 1)"
    );
    // Resolved foreground for the default ghostty palette index 1 is #cc6666.
    assert_eq!(
        cell0.fg.map(|c| (c.r, c.g, c.b)),
        Some((0xcc, 0x66, 0x66)),
        "resolved fg is the default-palette red"
    );

    // Cell at row 0 col 4: 'u' from ESC[4m -> underline set, not bold.
    let cell4 = &snap.viewport[0][4];
    assert_eq!(cell4.ch, Some('u'), "fifth cell is 'u'");
    assert!(!cell4.style.bold, "fifth cell is not bold");
    assert!(cell4.style.underline.is_some(), "fifth cell is underlined");

    // Print a proof line so the build log shows the round-trip, matching the
    // spike's PROOF-OK.
    println!(
        "PROOF-OK cursor={:?} pos={:?} cell0={:?} fg={:?} cell4_underline={:?}",
        snap.cursor.visual_style,
        snap.cursor.viewport,
        cell0.ch,
        cell0.fg,
        cell4.style.underline,
    );
}

#[test]
fn resize_changes_viewport_dimensions() {
    let mut term = Terminal::new(10, 20, 100).expect("create terminal");
    let before = term.render().expect("render before resize");
    assert_eq!((before.rows, before.cols), (10, 20));

    term.resize(12, 40).expect("resize");
    let after = term.render().expect("render after resize");
    assert_eq!((after.rows, after.cols), (12, 40), "viewport tracks resize");
    assert_eq!(after.viewport.len(), 12, "row count matches new height");
}

#[test]
fn decckm_tracks_application_cursor_keys() {
    let mut term = Terminal::new(24, 80, 0).expect("create terminal");
    assert!(
        !term
            .application_cursor_keys()
            .expect("query DECCKM at start"),
        "cursor keys default to normal mode"
    );

    // ESC[?1h sets DECCKM (what ncurses/vim emit via `smkx` on entry).
    term.vt_write(b"\x1b[?1h");
    assert!(
        term.application_cursor_keys().expect("query DECCKM after set"),
        "ESC[?1h enables application cursor keys"
    );

    // ESC[?1l resets it (the `rmkx` on exit).
    term.vt_write(b"\x1b[?1l");
    assert!(
        !term
            .application_cursor_keys()
            .expect("query DECCKM after reset"),
        "ESC[?1l restores normal cursor keys"
    );
}
