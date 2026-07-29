//! Semantic-prompt marks (OSC 133) and the full-text dump: the accessors a
//! server-side terminal needs for prompt-jump navigation over scrollback and
//! select-all copy (ix#8013, ix#8014).

use ix_vt::{RowLocation, RowSemanticPrompt, Terminal, scrollback_bytes_for_lines};

#[test]
fn osc_133_marks_prompt_rows_across_scrollback() {
    let mut term = Terminal::new(5, 40, scrollback_bytes_for_lines(1000, 40)).expect("create terminal");

    // Two shell round-trips overflowing the 5-row grid into scrollback. The
    // marks are what real shell integration emits: `133;A` opens the prompt,
    // `133;B` ends it (input begins), `133;C` starts command output.
    term.vt_write(b"\x1b]133;A\x07$ \x1b]133;B\x07first-command\r\n\x1b]133;C\x07");
    for i in 0..8 {
        term.vt_write(format!("output-{i}\r\n").as_bytes());
    }
    term.vt_write(b"\x1b]133;A\x07$ \x1b]133;B\x07");

    // Screen coordinates cover history: row 0 is the oldest (first prompt).
    assert_eq!(
        term.row_semantic_prompt(RowLocation::Screen(0))
            .expect("query oldest row"),
        RowSemanticPrompt::Prompt,
        "first prompt row is marked"
    );
    assert_eq!(
        term.row_semantic_prompt(RowLocation::Screen(1))
            .expect("query output row"),
        RowSemanticPrompt::None,
        "command output is unmarked"
    );

    // The live prompt row: find it by scanning screen rows for the second
    // mark below the first.
    let scrollbar = term.scrollbar().expect("scrollbar");
    let marked: Vec<u64> = (0..scrollbar.total)
        .filter(|&y| {
            let y = u32::try_from(y).expect("test rows fit u32");
            term.row_semantic_prompt(RowLocation::Screen(y))
                .expect("query")
                == RowSemanticPrompt::Prompt
        })
        .collect();
    assert_eq!(marked.len(), 2, "both prompts are marked, one in history");
    assert_eq!(marked[0], 0);
    assert_eq!(marked[1], 9, "second prompt after 8 output rows");
}

#[test]
fn out_of_range_row_is_an_error_not_a_default() {
    let term = Terminal::new(5, 40, scrollback_bytes_for_lines(1000, 40)).expect("create terminal");
    assert!(
        term.row_semantic_prompt(RowLocation::Screen(100_000))
            .is_err(),
        "an unreachable row reports invalid, so callers can stop a scan"
    );
}

#[test]
fn dump_text_covers_scrollback_and_active_screen() {
    let mut term = Terminal::new(5, 40, scrollback_bytes_for_lines(1000, 40)).expect("create terminal");
    for i in 0..20 {
        term.vt_write(format!("line-{i}\r\n").as_bytes());
    }
    term.vt_write(b"$ tail-prompt");

    let text = term.dump_text().expect("dump");
    assert!(text.contains("line-0"), "oldest scrollback row is included");
    assert!(text.contains("line-19"), "recent row is included");
    assert!(text.contains("$ tail-prompt"), "active row is included");
    assert!(
        !text.contains('\x1b'),
        "plain dump carries no escape sequences"
    );
    assert!(
        !text.lines().any(|line| line.ends_with(' ')),
        "trailing whitespace is trimmed"
    );
}

#[test]
fn dump_text_joins_soft_wrapped_lines() {
    let mut term = Terminal::new(5, 10, scrollback_bytes_for_lines(1000, 10)).expect("create terminal");
    // 25 columns of text through a 10-column grid: soft-wraps twice.
    term.vt_write(b"abcdefghijklmnopqrstuvwxy");
    let text = term.dump_text().expect("dump");
    assert!(
        text.contains("abcdefghijklmnopqrstuvwxy"),
        "soft-wrapped physical rows are unwrapped into the logical line: {text:?}"
    );
}

#[test]
fn empty_terminal_dumps_cleanly() {
    let term = Terminal::new(5, 40, scrollback_bytes_for_lines(1000, 40)).expect("create terminal");
    let text = term.dump_text().expect("dump");
    assert_eq!(text.trim(), "", "an empty terminal dumps no content");
}
