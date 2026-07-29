//! `max_scrollback_bytes` is a byte budget and `scrollback_bytes_for_lines` is
//! the only honest way to reach it from a row count (ix#9031).
//!
//! Both tests here exist because libghostty-vt's own header calls the field
//! "Maximum number of lines to keep in scrollback history", which is wrong, and
//! nothing in the C API or the type system catches a caller who believes it.

use ix_vt::{Terminal, scrollback_bytes_for_lines};

/// Rows the terminal still holds after `lines` newline-terminated rows have
/// been fed through it, which is what its scrollbar reports as `total`.
fn rows_kept(cols: u16, max_scrollback_bytes: usize, lines: usize) -> u64 {
    let mut term = Terminal::new(24, cols, max_scrollback_bytes).expect("terminal");
    for i in 0..lines {
        term.vt_write(format!("line-{i}\r\n").as_bytes());
    }
    term.scrollbar().expect("scrollbar").total
}

/// The promise the helper makes: ask for N rows at a width, get at least N
/// rows. This is what fails if ghostty grows a page arena past the margin in
/// `ROW_BUDGET_COST_PERCENT`.
#[test]
fn scrollback_bytes_for_lines_delivers_the_rows_it_promises() {
    for (lines, cols) in [(10_000_usize, 80_u16), (10_000, 200), (2_000, 400), (40_000, 80)] {
        let budget = scrollback_bytes_for_lines(lines, cols);
        // Twice the ask, so the terminal is saturated and the number read back
        // is the limit rather than however much happened to be written.
        let kept = rows_kept(cols, budget, lines * 2);
        assert!(
            kept >= lines as u64,
            "{lines} rows at {cols} columns budgeted {budget} bytes and kept only {kept}"
        );
    }
}

/// The units, asserted the only way that survives a libghostty change: a row
/// count written into the byte field must NOT buy that many rows.
///
/// This is ix#9031 as measured. Every budget below the floor ghostty needs for
/// the active area is silently raised to it (`PageList.maxSize` returns
/// `@max(explicit_max_size, min_max_size)`), so a hundredfold increase in the
/// number a user typed bought exactly the same 637 rows.
#[test]
fn a_row_count_in_the_byte_field_buys_the_same_floor_whatever_it_says() {
    let kept = [1_000_usize, 10_000, 100_000, 1_000_000]
        .map(|budget| rows_kept(80, budget, 20_000));

    assert!(
        kept[0] < 2_000,
        "1000 bytes is one row of 80 columns; keeping {} rows would mean the \
         field had become a row count and scrollback_bytes_for_lines is now \
         over-budgeting by 1000x",
        kept[0]
    );
    assert!(
        kept.iter().all(|&n| n == kept[0]),
        "every budget under ghostty's floor clamps to the same row count, so \
         these should be equal: {kept:?}"
    );
}
