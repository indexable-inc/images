//! `OscTitleTracker` over ghostty's streaming OSC parser: it captures the
//! window title from OSC 0/2, ignores the icon-only OSC 1, and tolerates a
//! sequence split across feed boundaries.

use ix_vt::OscTitleTracker;

#[test]
fn captures_bel_terminated_title() {
    let mut t = OscTitleTracker::new().expect("create tracker");
    assert_eq!(t.title(), None, "no title before any input");

    // OSC 2 (set window title), BEL-terminated: ESC ] 2 ; hello BEL.
    t.feed(b"\x1b]2;hello\x07");
    assert_eq!(t.title(), Some("hello"));
}

#[test]
fn captures_st_terminated_title_and_osc0() {
    let mut t = OscTitleTracker::new().expect("create tracker");
    // OSC 0 sets icon *and* window title; ST-terminated: ESC ] 0 ; world ESC \.
    t.feed(b"\x1b]0;world\x1b\\");
    assert_eq!(t.title(), Some("world"));
}

#[test]
fn title_persists_and_updates_across_feeds() {
    let mut t = OscTitleTracker::new().expect("create tracker");
    // A title split across three feeds, including a mid-payload boundary and a
    // split ST terminator (ESC in one chunk, '\\' in the next).
    t.feed(b"\x1b]2;split-");
    t.feed(b"title\x1b");
    t.feed(b"\\");
    assert_eq!(t.title(), Some("split-title"));

    // Ordinary output between sequences leaves the title untouched.
    t.feed(b"some normal output\r\n");
    assert_eq!(t.title(), Some("split-title"));

    // A later title replaces the earlier one.
    t.feed(b"\x1b]2;second\x07");
    assert_eq!(t.title(), Some("second"));
}

#[test]
fn esc_terminates_an_unterminated_title() {
    let mut t = OscTitleTracker::new().expect("create tracker");
    // OSC 2 with no BEL/ST, directly followed by a CSI: ghostty dispatches the
    // title on the ESC, so it must be captured (not dropped).
    t.feed(b"\x1b]2;vim\x1b[2J");
    assert_eq!(t.title(), Some("vim"));
}

#[test]
fn invalid_utf8_title_is_ignored() {
    let mut t = OscTitleTracker::new().expect("create tracker");
    t.feed(b"\x1b]2;good\x07");
    // A title with an invalid UTF-8 byte is skipped, not lossily replaced, so
    // the previous title stands.
    t.feed(b"\x1b]2;\xff\x07");
    assert_eq!(t.title(), Some("good"));
}

#[test]
fn can_aborts_an_in_progress_title() {
    let mut t = OscTitleTracker::new().expect("create tracker");
    t.feed(b"\x1b]2;keep\x07");
    // A CAN (0x18) mid-payload aborts the control string, so this title is
    // dropped, not captured, and the previous one stands.
    t.feed(b"\x1b]2;abor\x18ted\x07");
    assert_eq!(t.title(), Some("keep"));
}

#[test]
fn icon_only_osc1_does_not_change_title() {
    let mut t = OscTitleTracker::new().expect("create tracker");
    t.feed(b"\x1b]2;real-title\x07");
    // OSC 1 sets the icon name, not the window title.
    t.feed(b"\x1b]1;icon-name\x07");
    assert_eq!(
        t.title(),
        Some("real-title"),
        "icon-only OSC 1 must not overwrite the window title"
    );
}
