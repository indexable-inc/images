//! The row wrap flag and the cell wide tag: the two things a reflow needs.
//!
//! Both are ghostty's answers, not ours, and they are only useful if the
//! wrapper reports them for the same rows and columns ghostty put them on.
//! These tests pin that correspondence, because a reflow reading them one
//! row off would rejoin lines that were never joined and nothing else in the
//! system would notice.

use ix_vt::{CellWide, Terminal, scrollback_bytes_for_lines};

/// Rows the terminal broke because it ran out of columns are marked; the row
/// the program ended with a newline is not.
#[test]
fn soft_wrapped_rows_are_marked_and_hard_ones_are_not() {
    let mut term =
        Terminal::new(24, 10, scrollback_bytes_for_lines(1000, 10)).expect("create terminal");
    // 25 characters across a 10 column grid: two full rows that continue,
    // then five characters that do not.
    term.vt_write(b"abcdefghijklmnopqrstuvwxy");

    let snap = term.render().expect("render snapshot");
    let wrapped: Vec<bool> = snap
        .viewport
        .iter()
        .take(3)
        .map(|row| row.wrapped)
        .collect();
    assert_eq!(
        wrapped,
        vec![true, true, false],
        "the first two rows continue onto the next; the third is where the text stopped"
    );
}

/// A line the program ended itself is never marked, however full it is.
#[test]
fn an_exactly_full_row_ended_by_a_newline_is_not_wrapped() {
    let mut term =
        Terminal::new(24, 10, scrollback_bytes_for_lines(1000, 10)).expect("create terminal");
    term.vt_write(b"0123456789\r\nnext");

    let snap = term.render().expect("render snapshot");
    // The distinction this test exists for: the row is full to the last
    // column either way, so width alone cannot tell a soft wrap from a
    // deliberate line. Only the flag can.
    assert!(
        !snap.viewport[0].wrapped,
        "a full row followed by CR LF is a line the program ended"
    );
}

/// A two-column character occupies a `Wide` cell and the `SpacerTail` after
/// it, so a reader knows the pair is one character.
#[test]
fn a_double_width_character_tags_its_two_cells() {
    let mut term =
        Terminal::new(24, 10, scrollback_bytes_for_lines(1000, 10)).expect("create terminal");
    term.vt_write("漢字".as_bytes());

    let snap = term.render().expect("render snapshot");
    let tags: Vec<CellWide> = snap.viewport[0]
        .iter()
        .take(5)
        .map(|cell| cell.wide)
        .collect();
    assert_eq!(
        tags,
        vec![
            CellWide::Wide,
            CellWide::SpacerTail,
            CellWide::Wide,
            CellWide::SpacerTail,
            CellWide::Narrow
        ],
        "each kanji claims two columns and says which is which"
    );
}

/// A two-column character that will not fit in the last column leaves that
/// column as padding and starts on the next row. Nothing about the blank cell
/// says the program printed a space, and the tag is what says so.
#[test]
fn a_double_width_character_that_does_not_fit_leaves_a_spacer_head() {
    let mut term =
        Terminal::new(24, 10, scrollback_bytes_for_lines(1000, 10)).expect("create terminal");
    // Nine narrow characters leave one column, which a kanji cannot use.
    term.vt_write("123456789漢".as_bytes());

    let snap = term.render().expect("render snapshot");
    assert_eq!(
        snap.viewport[0][9].wide,
        CellWide::SpacerHead,
        "the last column is padding the terminal inserted, not output"
    );
    assert!(snap.viewport[0].wrapped, "the line continues below");
    assert_eq!(
        snap.viewport[1][0].wide,
        CellWide::Wide,
        "the kanji starts the next row whole"
    );
}
