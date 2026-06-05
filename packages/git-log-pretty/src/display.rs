//! Format a commit's summary line and its changed-file tree for the terminal.

use std::fmt::Write;

use anstyle::{Ansi256Color, AnsiColor, Color, Style};

use crate::avatar::Avatar;
use crate::palette::{self, Theme};
use crate::tree;
use crate::{git, time};

/// A conventional-commit summary split into its `type`, optional `scope`, and
/// description, e.g. `feat(api): add route`. The description keeps its leading
/// space so it renders directly after the chip.
struct Conventional<'a> {
    commit_type: &'a str,
    scope: Option<&'a str>,
    description: &'a str,
}

/// Parse a conventional-commit summary of the form `type(scope): description`
/// or `type: description`. The type must be alphabetic; anything else returns
/// `None` so the raw summary renders unchanged.
fn parse_conventional(summary: &str) -> Option<Conventional<'_>> {
    let (prefix, description) = summary.split_once(':')?;

    let (commit_type, scope) = match prefix.split_once('(') {
        Some((commit_type, rest)) => (commit_type, Some(rest.strip_suffix(')')?)),
        None => (prefix, None),
    };

    let type_ok = !commit_type.is_empty() && commit_type.chars().all(|c| c.is_ascii_alphabetic());
    type_ok.then_some(Conventional {
        commit_type,
        scope,
        description,
    })
}

/// Render a commit summary, painting a hashed background chip on a recognized
/// conventional-commit type and dimming any scope. Non-conventional summaries
/// pass through unstyled.
fn format_summary(summary: &str, theme: Theme) -> String {
    let Some(parsed) = parse_conventional(summary) else {
        return summary.to_string();
    };

    let chip = Style::new()
        .bg_color(Some(Color::Rgb(palette::hashed_chip_background(
            parsed.commit_type,
            theme,
        ))))
        .fg_color(Some(Color::Rgb(palette::chip_foreground(theme))))
        .bold();
    let chip = palette::paint(chip, parsed.commit_type);

    let chip = chip.as_str();
    let description = parsed.description;
    parsed.scope.map_or_else(
        || format!("{chip}{description}"),
        |scope| {
            let scope = palette::paint(palette::fg(Color::Rgb(palette::GRAY)), scope);
            format!("{chip} {scope}{description}")
        },
    )
}

/// A commit's two rendered pieces: the header line and the changed-file tree.
struct CommitBlock {
    /// The `shorthash summary • when` header line.
    header: String,
    /// The indented changed-file tree, possibly empty.
    tree: String,
}

/// Build a commit's two rendered pieces: the `shorthash summary • when` header
/// line and the (possibly empty) indented changed-file tree.
fn commit_block(ahead: &git::AheadCommit<'_>, theme: Theme) -> color_eyre::eyre::Result<CommitBlock> {
    let commit = &ahead.commit;
    let short = commit.id().to_string().chars().take(7).collect::<String>();
    let summary = commit.summary().unwrap_or("<no message>").trim();
    let when = time::relative(commit.time().seconds())?;

    let yellow = palette::fg(Color::Ansi(AnsiColor::Yellow));
    let dim = palette::fg(Color::Ansi256(Ansi256Color(8)));

    let header = format!(
        "  {short} {summary} {bullet} {when}",
        short = palette::paint(yellow, &short),
        summary = format_summary(summary, theme),
        bullet = palette::paint(dim, "•"),
        when = palette::paint(dim, &when),
    );
    let icons = tree::render(&ahead.changed_files, theme);
    Ok(CommitBlock { header, tree: icons })
}

/// Write the plain header / tree / blank-line form (no avatar).
fn write_plain(out: &mut dyn std::io::Write, header: &str, icons: &str) -> color_eyre::eyre::Result<()> {
    writeln!(out, "{header}")?;
    if !icons.is_empty() {
        writeln!(out, "{icons}")?;
    }
    writeln!(out)?;
    Ok(())
}

/// Write one commit block to `out`: a `shorthash summary • relative-time` line
/// followed by the indented changed-file tree and a trailing blank line.
pub fn print_commit(
    out: &mut dyn std::io::Write,
    ahead: &git::AheadCommit<'_>,
    theme: Theme,
) -> color_eyre::eyre::Result<()> {
    let CommitBlock { header, tree: icons } = commit_block(ahead, theme)?;
    write_plain(out, &header, &icons)
}

/// Width in cells of an avatar `rows` tall.
///
/// Cells are about twice as tall as wide, so the image spans `2 * rows` columns
/// to stay roughly square. The caller transmits the image into this same box.
#[must_use]
pub const fn avatar_cols(rows: u32) -> u32 {
    rows * 2
}

/// Write one commit block with the author's avatar in the left gutter.
///
/// The avatar is drawn with kitty [Unicode placeholder] cells: the pixels were
/// already sent once by the caller via [`kitty::transmit_virtual`], so here we
/// only print placeholder text tagged with the image id. Because the gutter is
/// ordinary left-to-right text (no cursor motion), the block scrolls and pages
/// like any other output. With no avatar this falls back to the plain form.
///
/// [Unicode placeholder]: https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders
pub fn print_commit_with_avatar(
    out: &mut dyn std::io::Write,
    ahead: &git::AheadCommit<'_>,
    theme: Theme,
    avatar: Option<&Avatar>,
    rows: u32,
) -> color_eyre::eyre::Result<()> {
    let CommitBlock { header, tree: icons } = commit_block(ahead, theme)?;

    let Some(avatar) = avatar.filter(|_| rows > 0) else {
        return write_plain(out, &header, &icons);
    };

    let mut lines = vec![header.as_str()];
    if !icons.is_empty() {
        lines.extend(icons.lines());
    }
    write!(out, "{}", compose_avatar_block(&lines, avatar.id, rows))?;
    Ok(())
}

/// Lay out `lines` beside an avatar `rows` cells tall, returning the block as
/// text terminated by a blank separator line.
///
/// The first `rows` lines carry the kitty placeholder cells for image
/// `avatar_id` (one image row each); any further lines are padded with spaces so
/// the text stays aligned past the image. The block always spans at least `rows`
/// lines so the image is never clipped vertically. Pure, so it is unit-tested
/// directly without a real commit.
fn compose_avatar_block(lines: &[&str], avatar_id: u32, rows: u32) -> String {
    let cols = avatar_cols(rows);
    // The image is `cols` wide; one more column separates it from the text.
    let gutter = cols as usize + 1;
    // `rows` is a `u32` terminal-row count, lossless to `usize` on 64-bit.
    let count = lines.len().max(rows as usize);

    let mut buf = String::new();
    for index in 0..count {
        let content = lines.get(index).copied().unwrap_or("");
        if let Ok(row) = u32::try_from(index)
            && row < rows
        {
            // Placeholder cells, a one-column gap, then the text.
            let _ = writeln!(buf, "{} {content}", kitty::placeholder_row(avatar_id, row, cols));
        } else {
            // Past the image: pad the gutter with spaces so the text stays aligned.
            let _ = writeln!(buf, "{:gutter$}{content}", "");
        }
    }
    buf.push('\n');
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(styled: &str) -> String {
        String::from_utf8(strip_ansi_escapes::strip(styled)).unwrap()
    }

    #[test]
    fn parses_type_scope_description() {
        let parsed = parse_conventional("feat(api): add route").unwrap();
        assert_eq!(parsed.commit_type, "feat");
        assert_eq!(parsed.scope, Some("api"));
        assert_eq!(parsed.description, " add route");
    }

    #[test]
    fn parses_type_without_scope() {
        let parsed = parse_conventional("fix: bug").unwrap();
        assert_eq!(parsed.commit_type, "fix");
        assert_eq!(parsed.scope, None);
    }

    #[test]
    fn rejects_non_conventional_summaries() {
        assert!(parse_conventional("just a message").is_none());
        assert!(parse_conventional("123: numeric type").is_none());
    }

    #[test]
    fn non_conventional_summary_passes_through_plain() {
        assert_eq!(format_summary("plain message", Theme::Dark), "plain message");
    }

    #[test]
    fn conventional_summary_keeps_its_text() {
        let rendered = plain(&format_summary("feat(api): add route", Theme::Dark));
        assert_eq!(rendered, "feat api add route");
    }

    #[test]
    fn avatar_block_tags_each_image_row_with_placeholders() {
        let block = compose_avatar_block(&["header", "tree"], 42, 2);
        let lines: Vec<&str> = block.lines().collect();
        // Two image rows beside the two content lines, then a blank separator.
        assert!(lines[0].contains(kitty::PLACEHOLDER), "row 0 needs placeholder cells");
        assert!(lines[1].contains(kitty::PLACEHOLDER), "row 1 needs placeholder cells");
        assert!(lines[0].contains("header") && lines[1].contains("tree"));
        assert_eq!(block.lines().last(), Some(""), "block ends with a blank line");
    }

    #[test]
    fn avatar_block_pads_lines_below_the_image() {
        // One image row, two content lines: the second line clears the gutter
        // with spaces (no placeholder) so it stays aligned under the text.
        let block = compose_avatar_block(&["header", "extra"], 7, 1);
        let lines: Vec<&str> = block.lines().collect();
        assert!(lines[0].contains(kitty::PLACEHOLDER));
        assert!(!lines[1].contains(kitty::PLACEHOLDER), "padded line has no placeholder");
        // gutter = avatar_cols(1) + 1 = 3 spaces before the text.
        assert!(lines[1].starts_with("   extra"), "misaligned pad: {:?}", lines[1]);
    }

    #[test]
    fn avatar_block_never_clips_image_vertically() {
        // A single content line still emits two rows so a 2-row image fits.
        let block = compose_avatar_block(&["only"], 9, 2);
        let placeholder_rows =
            block.lines().filter(|line| line.contains(kitty::PLACEHOLDER)).count();
        assert_eq!(placeholder_rows, 2, "both image rows must be drawn");
    }
}
