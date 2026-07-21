//! Mode-state accessors for input routing (`mouse_reporting`, `sgr_mouse`,
//! `alternate_screen`, `scrollbar`): the queries a server-side input driver
//! needs to decide whether wheel input scrolls scrollback, becomes arrow
//! keys, or is forwarded to the application as encoded mouse events.

use ix_vt::{MouseReporting, Terminal};

#[test]
fn mouse_reporting_follows_decset_and_strongest_wins() {
    let mut term = Terminal::new(24, 80, 1000).expect("create terminal");
    assert_eq!(term.mouse_reporting().expect("query"), MouseReporting::None);
    assert!(!term.sgr_mouse().expect("query"));

    // vim's usual request: normal tracking + button drag + SGR encoding.
    term.vt_write(b"\x1b[?1000h\x1b[?1002h\x1b[?1006h");
    assert_eq!(
        term.mouse_reporting().expect("query"),
        MouseReporting::Button,
        "1002 outranks 1000"
    );
    assert!(term.sgr_mouse().expect("query"));

    term.vt_write(b"\x1b[?1003h");
    assert_eq!(term.mouse_reporting().expect("query"), MouseReporting::Any);

    // Reset everything: back to terminal-owned mouse.
    term.vt_write(b"\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?1006l");
    assert_eq!(term.mouse_reporting().expect("query"), MouseReporting::None);
    assert!(!term.sgr_mouse().expect("query"));
}

#[test]
fn alternate_screen_tracks_1049_and_1047() {
    let mut term = Terminal::new(24, 80, 1000).expect("create terminal");
    assert!(!term.alternate_screen().expect("query"));

    term.vt_write(b"\x1b[?1049h");
    assert!(term.alternate_screen().expect("query"), "1049 enters alt");
    term.vt_write(b"\x1b[?1049l");
    assert!(!term.alternate_screen().expect("query"), "1049 leaves alt");

    term.vt_write(b"\x1b[?1047h");
    assert!(term.alternate_screen().expect("query"), "1047 enters alt");
    term.vt_write(b"\x1b[?1047l");
    assert!(!term.alternate_screen().expect("query"), "1047 leaves alt");
}

#[test]
fn scrollbar_reports_history_and_bottom() {
    let mut term = Terminal::new(4, 20, 100).expect("create terminal");

    let bar = term.scrollbar().expect("query");
    assert_eq!(bar.len, 4, "viewport height");
    assert_eq!(
        bar.offset + bar.len,
        bar.total,
        "fresh terminal viewport is at the bottom"
    );
    let empty_total = bar.total;

    // Push 8 lines through a 4-row grid: history grows past the viewport.
    for i in 0..8 {
        term.vt_write(format!("line {i}\r\n").as_bytes());
    }
    let bar = term.scrollbar().expect("query");
    assert!(bar.total > empty_total, "output grew the scrollable area");
    assert_eq!(
        bar.offset + bar.len,
        bar.total,
        "viewport follows the live bottom"
    );

    term.scroll_viewport(ix_vt::ScrollViewport::Top);
    let bar = term.scrollbar().expect("query");
    assert_eq!(bar.offset, 0, "scrolled to the oldest row");
    assert!(
        bar.offset + bar.len < bar.total,
        "viewport is no longer at the bottom"
    );

    term.scroll_viewport(ix_vt::ScrollViewport::Bottom);
    let bar = term.scrollbar().expect("query");
    assert_eq!(bar.offset + bar.len, bar.total, "back at the bottom");
}
