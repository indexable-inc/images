//! Shared terminal progress styling for ix command-line tools.
//!
//! One owner for the glyphs, colors, and templates every ix CLI uses to draw
//! progress, so `search`, `dag-runner`, and future commands render the
//! same shape instead of each hand-rolling an [`indicatif`] template. Pick a
//! style here, then set the per-run label with [`ProgressBar::set_prefix`] and
//! the per-run status with [`ProgressBar::set_message`].
//!
//! [`ProgressBar::set_prefix`]: indicatif::ProgressBar::set_prefix
//! [`ProgressBar::set_message`]: indicatif::ProgressBar::set_message

use indicatif::ProgressStyle;

/// Progress-bar cell glyphs in the order `indicatif`'s `progress_chars` reads
/// them: the first glyph fills a complete cell, the last marks an empty cell,
/// and the middle glyphs render the partially-filled head cell from 7/8 down to
/// 1/8.
///
/// A full block, seven fractional blocks, and a light-shade track make the fill
/// advance one-eighth of a cell at a time and keep the whole bar one contiguous
/// shape, replacing the segmented `=>-` arrow look. The empty glyph is `░`
/// rather than a space so the track stays visible.
const BAR_CHARS: &str = "█▉▊▋▌▍▎▏░";

/// Spinner frames: a braille wheel with a trailing blank frame so the final
/// tick clears cleanly. Shared so every ix command spins the same way.
const TICK_CHARS: &str = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ";

/// A determinate progress bar: a green braille spinner, the caller's `{prefix}`
/// label, a `pos/len` counter, the contiguous block bar, and elapsed time.
///
/// `accent` is an `indicatif` color name applied to the filled run over a
/// `blue` track (for example `"cyan"` or `"magenta"`); callers vary it to mark
/// distinct phases. The returned style is reusable across bars; set the label
/// per run with [`ProgressBar::set_prefix`] so the template itself stays fixed.
///
/// # Panics
///
/// Never in practice: the template is constructed from a fixed shape and a
/// color name, so [`ProgressStyle::with_template`] always parses it. The
/// `expect` guards against an edit that makes the template malformed.
///
/// [`ProgressBar::set_prefix`]: indicatif::ProgressBar::set_prefix
#[must_use]
pub fn bar(accent: &str) -> ProgressStyle {
    // Braces are doubled so `format!` emits literal `{...}` for indicatif to
    // parse at render time; only `{accent}` is a real format argument. Building
    // the template through `format!` rather than a string literal also sidesteps
    // clippy's `literal_string_with_formatting_args` false positive, which reads
    // indicatif's `{spinner:.green}` keys as stray `format!` placeholders.
    let template = format!(
        "{{spinner:.green}} {{prefix}} {{pos}}/{{len}} {{wide_bar:.{accent}/blue}} {{elapsed}}"
    );
    ProgressStyle::with_template(&template)
        .expect("progress bar template is valid")
        .progress_chars(BAR_CHARS)
        .tick_chars(TICK_CHARS)
}

/// An indeterminate spinner for work with no known total.
///
/// Renders a cyan braille spinner, a bold `{prefix}` label, a `{wide_msg}`
/// status line, and dimmed elapsed time, sized for one running task in a
/// [`MultiProgress`] group. Set the label with [`ProgressBar::set_prefix`] and
/// the status with [`ProgressBar::set_message`].
///
/// # Panics
///
/// Never in practice; see [`bar`].
///
/// [`MultiProgress`]: indicatif::MultiProgress
/// [`ProgressBar::set_prefix`]: indicatif::ProgressBar::set_prefix
/// [`ProgressBar::set_message`]: indicatif::ProgressBar::set_message
#[must_use]
pub fn spinner() -> ProgressStyle {
    // See `bar` for why this clippy lint is a false positive on indicatif keys.
    #[allow(clippy::literal_string_with_formatting_args)]
    ProgressStyle::with_template("{spinner:.cyan} {prefix:.bold} {wide_msg} {elapsed:.dim}")
        .expect("spinner template is valid")
        .tick_chars(TICK_CHARS)
}

#[cfg(test)]
mod tests {
    use super::{bar, spinner};

    // The `expect` calls inside `bar`/`spinner` turn a malformed template into a
    // panic; exercising both builders proves the templates parse, which is the
    // one invariant the type system cannot check.
    #[test]
    fn styles_build() {
        let _ = bar("cyan");
        let _ = bar("magenta");
        let _ = spinner();
    }
}
