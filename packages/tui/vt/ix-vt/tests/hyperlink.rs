//! OSC 8 hyperlink URI exposure through the render state (index#3835).
//!
//! The anchor text deliberately differs from the URI ("click me" vs
//! <https://example.com>), the exact case plain-URL detection cannot cover
//! (indexable-inc/ix#8008). Exercises the fork's C API end-to-end against
//! the real patched library: per-cell URIs on anchor cells, `None`
//! elsewhere, distinct adjacent links kept apart, and the copied URI
//! surviving later terminal writes.

use ix_vt::Terminal;

/// OSC 8 anchor: `ESC ] 8 ; params ; URI ST text ESC ] 8 ; ; ST`.
fn osc8(uri: &str, text: &str) -> Vec<u8> {
    format!("\x1b]8;;{uri}\x1b\\{text}\x1b]8;;\x1b\\").into_bytes()
}

#[test]
fn anchor_cells_carry_the_uri() {
    let mut term = Terminal::new(24, 80, 1000).expect("create terminal");
    term.vt_write(&osc8("https://example.com", "click me"));

    let snap = term.render().expect("render snapshot");
    let row = &snap.viewport[0];

    // Every cell of the 8-column anchor text carries the URI...
    for (x, cell) in row.iter().enumerate().take("click me".len()) {
        assert_eq!(
            cell.hyperlink.as_deref(),
            Some("https://example.com"),
            "cell {x} carries the link URI"
        );
    }
    // ...and the text is the anchor text, not the URI.
    let text: String = row
        .iter()
        .take("click me".len())
        .map(|c| c.ch.unwrap_or(' '))
        .collect();
    assert_eq!(text, "click me");

    // The cell after the anchor has no hyperlink.
    assert_eq!(row["click me".len()].hyperlink, None);
}

#[test]
fn adjacent_distinct_links_stay_distinct() {
    let mut term = Terminal::new(24, 80, 1000).expect("create terminal");
    let mut bytes = osc8("https://a.example", "AA");
    bytes.extend_from_slice(&osc8("https://b.example", "BB"));
    term.vt_write(&bytes);

    let snap = term.render().expect("render snapshot");
    let row = &snap.viewport[0];
    assert_eq!(row[0].hyperlink.as_deref(), Some("https://a.example"));
    assert_eq!(row[1].hyperlink.as_deref(), Some("https://a.example"));
    assert_eq!(row[2].hyperlink.as_deref(), Some("https://b.example"));
    assert_eq!(row[3].hyperlink.as_deref(), Some("https://b.example"));
    assert_eq!(row[4].hyperlink, None);
}

#[test]
fn plain_rows_have_no_links() {
    let mut term = Terminal::new(24, 80, 1000).expect("create terminal");
    term.vt_write(b"no links here, not even https://example.com as text");

    let snap = term.render().expect("render snapshot");
    assert!(
        snap.viewport
            .iter()
            .flatten()
            .all(|cell| cell.hyperlink.is_none()),
        "plain text (even URL-shaped) yields no hyperlink metadata"
    );
}

#[test]
fn snapshot_uri_is_owned_not_borrowed_from_the_terminal() {
    let mut term = Terminal::new(24, 80, 1000).expect("create terminal");
    term.vt_write(&osc8("https://example.com", "link"));
    let snap = term.render().expect("render snapshot");

    // Scroll the link away and overwrite the screen; the earlier snapshot
    // must still hold the URI (the wrapper copies out of the C state).
    for _ in 0..200 {
        term.vt_write(b"filler\r\n");
    }
    assert_eq!(
        snap.viewport[0][0].hyperlink.as_deref(),
        Some("https://example.com")
    );
}
