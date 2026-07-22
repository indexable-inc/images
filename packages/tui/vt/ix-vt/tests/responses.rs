//! Terminal query responses drain back out of the engine (ix#8117).
//!
//! Without a registered `write_pty` callback libghostty-vt silently drops
//! every sequence that requires a reply, so anything that queries the
//! terminal at startup (reedline reads the cursor position before drawing
//! its prompt) hangs forever. These tests defend the round trip: feed a
//! query, drain the reply.

use ix_vt::Terminal;

#[test]
fn dsr_cursor_position_reply_is_drained() {
    let mut terminal = Terminal::new(24, 80, 100).expect("terminal");
    terminal.vt_write(b"hi");
    // "hi" moves the cursor to column 3 (1-indexed) on row 1.
    terminal.vt_write(b"\x1b[6n");
    assert_eq!(terminal.drain_responses(), b"\x1b[1;3R");
    assert!(
        terminal.drain_responses().is_empty(),
        "drain empties the buffer"
    );
}

#[test]
fn da1_and_decrqm_get_replies() {
    let mut terminal = Terminal::new(24, 80, 100).expect("terminal");
    // DA1 (CSI c): the reply shape is CSI ? ... c; the feature list is
    // ghostty's to choose, so only the frame is asserted.
    terminal.vt_write(b"\x1b[c");
    let reply = terminal.drain_responses();
    assert!(
        reply.starts_with(b"\x1b[?") && reply.ends_with(b"c"),
        "DA1 reply {reply:?} is not CSI ? ... c"
    );

    // DECRQM for bracketed paste (mode 2004, reset): CSI ? 2004 ; 2 $ y.
    terminal.vt_write(b"\x1b[?2004$p");
    assert_eq!(terminal.drain_responses(), b"\x1b[?2004;2$y");
}

#[test]
fn plain_output_produces_no_responses() {
    let mut terminal = Terminal::new(24, 80, 100).expect("terminal");
    terminal.vt_write(b"plain text, no queries\r\n");
    assert!(terminal.drain_responses().is_empty());
}
