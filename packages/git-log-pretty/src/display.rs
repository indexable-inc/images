//! Format a commit's summary line and its changed-file tree for the terminal.

use anstyle::{Ansi256Color, AnsiColor, Color, Style};

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

/// Print one commit block: a `shorthash summary • relative-time` line followed
/// by the indented changed-file tree and a trailing blank line.
pub fn print_commit(ahead: &git::AheadCommit<'_>, theme: Theme) -> color_eyre::eyre::Result<()> {
    let commit = &ahead.commit;
    let short = commit
        .id()
        .to_string()
        .chars()
        .take(7)
        .collect::<String>();
    let summary = commit.summary().unwrap_or("<no message>").trim();
    let when = time::relative(commit.time().seconds())?;

    let yellow = palette::fg(Color::Ansi(AnsiColor::Yellow));
    let dim = palette::fg(Color::Ansi256(Ansi256Color(8)));

    println!(
        "  {short} {summary} {bullet} {when}",
        short = palette::paint(yellow, &short),
        summary = format_summary(summary, theme),
        bullet = palette::paint(dim, "•"),
        when = palette::paint(dim, &when),
    );

    let icons = tree::render(&ahead.changed_files, theme);
    if !icons.is_empty() {
        println!("{icons}");
    }
    println!();

    Ok(())
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
}
