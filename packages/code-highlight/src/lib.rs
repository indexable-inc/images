//! Tree-sitter syntax highlighting for source files, rendered as ANSI text.
//!
//! The crate owns one job: turn a source string plus a language hint (a file
//! path or extension) into colored terminal output. It resolves the language
//! through [`file_language`], wraps the official [`tree_sitter_highlight`] crate
//! and a curated set of `tree-sitter-<lang>` grammars, maps the standard
//! highlight capture names to a small [`anstyle`]-based theme, and renders ANSI
//! escapes when the caller asks for color.
//!
//! Two public entry points cover the shapes a snippet renderer needs:
//!
//! - [`highlight`] colors a whole file.
//! - [`highlight_lines`] colors a line range and prefixes a 1-based line-number
//!   gutter, the shape a search tool uses for context snippets.
//!
//! Unsupported languages, grammar build failures, and highlighter errors all
//! fall back to plain (uncolored) text rather than erroring, so a caller can
//! always render *something*. When `color` is `false` the output carries no
//! escape sequences at all, which is what the caller passes for `NO_COLOR` or a
//! non-TTY sink.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::LazyLock;

use anstyle::{Color, RgbColor, Style};
use tree_sitter_highlight::{Highlight, HighlightConfiguration, HighlightEvent, Highlighter};

pub use file_language::Language;

/// Capture names the theme understands, in a fixed order shared by every
/// [`HighlightConfiguration`]. The index a grammar reports for a capture is the
/// index into this slice, so the slice doubles as the capture-to-style key.
///
/// The names follow the conventional tree-sitter highlight taxonomy (the set
/// `tree-sitter highlight` and editors use). Dotted names such as
/// `function.method` let a grammar match a specific capture; the lookup in
/// [`style_for`] falls back from the most specific name to its prefix, so an
/// unstyled `function.macro` still picks up the `function` style.
const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "comment",
    "comment.documentation",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "escape",
    "function",
    "function.builtin",
    "function.macro",
    "function.method",
    "keyword",
    "label",
    "module",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.escape",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.member",
    "variable.parameter",
];

/// The grammar and highlights query for a language, or `None` when this crate
/// bundles no grammar for it.
///
/// Each grammar exports its highlights query under a slightly different constant
/// name (`HIGHLIGHTS_QUERY`, `HIGHLIGHT_QUERY`, or the block query for
/// Markdown), so the per-language arm names the right one. TypeScript and TSX
/// inherit the JavaScript highlights query: the TypeScript grammar's own query
/// only adds type-level rules and expects the ECMAScript rules to be present, so
/// the JS query is prepended. JavaScript and TSX additionally fold in the JSX
/// highlights query so embedded JSX colors instead of rendering plain.
///
/// [`Language`] is `#[non_exhaustive]`, so the wildcard arm covers any future
/// detection-only variant that has no grammar here: it resolves to plain text.
#[allow(
    clippy::too_many_lines,
    reason = "flat one-arm-per-language dispatch table; splitting it would hide the grammar-to-query mapping"
)]
fn grammar_query(language: Language) -> Option<(tree_sitter::Language, String)> {
    let pair = match language {
        Language::Rust => (
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::Python => (
            tree_sitter_python::LANGUAGE.into(),
            tree_sitter_python::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::JavaScript => (
            tree_sitter_javascript::LANGUAGE.into(),
            format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY
            ),
        ),
        Language::TypeScript => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
        ),
        Language::Tsx => (
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            format!(
                "{}\n{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
        ),
        Language::Go => (
            tree_sitter_go::LANGUAGE.into(),
            tree_sitter_go::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::C => (
            tree_sitter_c::LANGUAGE.into(),
            tree_sitter_c::HIGHLIGHT_QUERY.to_owned(),
        ),
        Language::Cpp => (
            tree_sitter_cpp::LANGUAGE.into(),
            tree_sitter_cpp::HIGHLIGHT_QUERY.to_owned(),
        ),
        Language::CSharp => (
            tree_sitter_c_sharp::LANGUAGE.into(),
            tree_sitter_c_sharp::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::Java => (
            tree_sitter_java::LANGUAGE.into(),
            tree_sitter_java::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::Scala => (
            tree_sitter_scala::LANGUAGE.into(),
            tree_sitter_scala::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::Swift => (
            tree_sitter_swift::LANGUAGE.into(),
            tree_sitter_swift::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::Ruby => (
            tree_sitter_ruby::LANGUAGE.into(),
            tree_sitter_ruby::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::Php => (
            tree_sitter_php::LANGUAGE_PHP.into(),
            tree_sitter_php::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::Lua => (
            tree_sitter_lua::LANGUAGE.into(),
            tree_sitter_lua::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::Haskell => (
            tree_sitter_haskell::LANGUAGE.into(),
            tree_sitter_haskell::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::Elixir => (
            tree_sitter_elixir::LANGUAGE.into(),
            tree_sitter_elixir::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::OCaml => (
            tree_sitter_ocaml::LANGUAGE_OCAML.into(),
            tree_sitter_ocaml::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::Html => (
            tree_sitter_html::LANGUAGE.into(),
            tree_sitter_html::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::Css => (
            tree_sitter_css::LANGUAGE.into(),
            tree_sitter_css::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::Json => (
            tree_sitter_json::LANGUAGE.into(),
            tree_sitter_json::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::Toml => (
            tree_sitter_toml_ng::LANGUAGE.into(),
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::Yaml => (
            tree_sitter_yaml::LANGUAGE.into(),
            tree_sitter_yaml::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::Sql => (
            tree_sitter_sequel::LANGUAGE.into(),
            tree_sitter_sequel::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::Nix => (
            tree_sitter_nix::LANGUAGE.into(),
            tree_sitter_nix::HIGHLIGHTS_QUERY.to_owned(),
        ),
        Language::Bash => (
            tree_sitter_bash::LANGUAGE.into(),
            tree_sitter_bash::HIGHLIGHT_QUERY.to_owned(),
        ),
        Language::Markdown => (
            tree_sitter_md::LANGUAGE.into(),
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.to_owned(),
        ),
        // A detection-only language with no bundled grammar renders as plain
        // text. Reachable only if `file_language` adds a variant ahead of a
        // grammar here.
        _ => return None,
    };
    Some(pair)
}

/// Builds a [`HighlightConfiguration`] for a language, or `None` when the
/// grammar is absent or its query fails to compile.
///
/// The injection and locals queries are left empty: this crate highlights one
/// language per file and resolves no injections, so the queries would do nothing
/// and only add per-grammar constant-name fragility.
fn build_config(language: Language) -> Option<HighlightConfiguration> {
    let (ts_language, highlights) = grammar_query(language)?;
    let mut config = HighlightConfiguration::new(ts_language, language.name(), &highlights, "", "")
        .inspect_err(|error| {
            // A query that fails to compile is a grammar-version skew bug, not a
            // user error; surface it once at startup rather than silently
            // dropping the language to plain text.
            eprintln!(
                "code-highlight: {} highlights query failed to compile: {error}",
                language.name()
            );
        })
        .ok()?;
    config.configure(HIGHLIGHT_NAMES);
    Some(config)
}

/// Process-wide cache of built highlight configurations.
///
/// [`HighlightConfiguration::new`] compiles the grammar's query, which is the
/// expensive part of highlighting, so each language is built once and reused. A
/// language whose config fails to build maps to `None` and is treated as
/// unsupported (plain-text fallback) for the rest of the process.
static CONFIGS: LazyLock<HashMap<Language, Option<HighlightConfiguration>>> = LazyLock::new(|| {
    Language::ALL
        .iter()
        .map(|&language| (language, build_config(language)))
        .collect()
});

/// Returns the cached config for a language, or `None` if it is unsupported or
/// failed to build.
fn config_for(language: Language) -> Option<&'static HighlightConfiguration> {
    CONFIGS.get(&language).and_then(Option::as_ref)
}

/// The `JetBrains` New UI Dark theme, expressed as `anstyle` styles keyed by
/// highlight capture name. Colors are 24-bit RGB; terminals without true-color
/// support degrade them to the nearest palette entry. The dark palette matches
/// the precedent highlighter (superglide's `JetBrainsNewDark`).
fn style_for_name(name: &str) -> Style {
    /// `#RRGGBB` to an `anstyle` foreground style.
    const fn fg(r: u8, g: u8, b: u8) -> Style {
        Style::new().fg_color(Some(Color::Rgb(RgbColor(r, g, b))))
    }

    match name {
        "keyword" | "constant.builtin" | "variable.builtin" => fg(0xCF, 0x8E, 0x6D), // orange
        "function" | "function.builtin" | "function.method" => fg(0x56, 0xA8, 0xF5), // blue
        "function.macro" | "attribute" => fg(0xB2, 0x00, 0xB2).bold(),               // macro/attr
        "type" | "type.builtin" | "constructor" | "module" => fg(0x6F, 0xAF, 0xBD),  // cyan-blue
        "string" | "string.special" | "string.escape" | "escape" => fg(0x6A, 0xAB, 0x73), // green
        "number" => fg(0x2A, 0xAC, 0xB8),                                            // cyan
        "comment" => fg(0x7A, 0x7E, 0x85).italic(),                                  // gray
        "comment.documentation" => fg(0x5F, 0x82, 0x6B).italic(),                    // green-gray
        "constant" => fg(0xC7, 0x7D, 0xBB).italic(),                                 // purple
        "property" | "variable.member" => fg(0xC7, 0x7D, 0xBB), // purple/magenta
        "tag" | "label" => fg(0xFF, 0xC6, 0x6D),                // yellow-orange
        // Plain identifiers, parameters, operators, and punctuation get the
        // foreground gray so ordinary code still reads as colored rather than
        // falling through to the terminal default and looking unhighlighted.
        "variable" | "variable.parameter" | "operator" | "punctuation" => fg(0xBC, 0xBE, 0xC4),
        _ => Style::new(),
    }
}

/// Resolves the style for a capture name, falling back from the most specific
/// dotted name to its prefixes (`function.method` to `function`).
fn style_for(name: &str) -> Style {
    let mut current = name;
    loop {
        let style = style_for_name(current);
        if style != Style::new() {
            return style;
        }
        match current.rfind('.') {
            Some(dot) => current = &current[..dot],
            None => return Style::new(),
        }
    }
}

/// Writes `text` to `out` wrapped in `style`'s ANSI escapes when `color` is set,
/// or raw when it is not. An empty or default style writes the text unchanged.
fn push_styled(out: &mut String, text: &str, style: Style, color: bool) {
    if color && style != Style::new() {
        // `anstyle::Style`'s `Display` renders the SGR prefix; `render_reset`
        // emits the matching reset. Both are infallible writes to a `String`.
        let _ = write!(out, "{style}{text}{reset}", reset = style.render_reset());
    } else {
        out.push_str(text);
    }
}

/// Resolves a language from a path or a bare extension/name.
///
/// Tries the full path first (so `uv.lock` resolves to TOML and `main.rs` to
/// Rust), then treats the whole string as a bare extension (so a caller that
/// only knows `"rs"` still resolves). Returns `None` when nothing matches.
fn detect(path_or_lang: &str) -> Option<Language> {
    Language::from_path(Path::new(path_or_lang)).or_else(|| Language::from_extension(path_or_lang))
}

/// Highlights a full source string and returns it as a single rendered block.
///
/// `path_or_lang` is the source path or bare extension used to pick a grammar;
/// pass the real file path when you have one so the filename and extension both
/// resolve. When `color` is `true` the output carries ANSI SGR escapes; when
/// `false` it is the input text unchanged.
///
/// Unsupported languages and any highlighter failure fall back to returning the
/// source verbatim, so this function never errors.
#[must_use]
pub fn highlight(path_or_lang: &str, source: &str, color: bool) -> String {
    let Some(language) = detect(path_or_lang) else {
        return source.to_owned();
    };
    render_spans(language, source, color).unwrap_or_else(|| source.to_owned())
}

/// Highlights a line range and prefixes a 1-based line-number gutter.
///
/// `start_line` is 1-based and inclusive; `num_lines` lines are emitted starting
/// there (clamped to the end of the file). The gutter is right-aligned to the
/// width of the largest line number in the range and separated from the code by
/// ` │ `. The gutter is dimmed when `color` is set.
///
/// This is the snippet shape a search tool renders for `-c` context: highlight
/// the whole file once, then slice the requested window so multi-line
/// constructs are colored with full-file context.
///
/// Like [`highlight`], unsupported languages and highlighter failures fall back
/// to plain (still gutter-prefixed) text, so this function never errors.
#[must_use]
pub fn highlight_lines(
    path_or_lang: &str,
    source: &str,
    start_line: usize,
    num_lines: usize,
    color: bool,
) -> String {
    let rendered = detect(path_or_lang)
        .and_then(|language| render_spans(language, source, color))
        .unwrap_or_else(|| source.to_owned());

    // Slice the rendered output by line. Splitting on '\n' is safe even with
    // ANSI escapes present: escapes never contain a newline, and each escape is
    // opened and closed within a single source span, so a span never straddles
    // a line boundary in a way that splits an escape. A trailing '\n' terminates
    // the final line rather than starting an empty one, so drop the single empty
    // tail element `split` leaves behind; a file with no trailing newline keeps
    // its last partial line.
    let mut lines: Vec<&str> = rendered.split('\n').collect();
    if rendered.ends_with('\n') {
        lines.pop();
    }
    let start = start_line.max(1);
    let total = lines.len();
    if start > total {
        return String::new();
    }
    let end = start.saturating_add(num_lines).min(total.saturating_add(1));
    let last_number = end.saturating_sub(1);
    let width = decimal_width(last_number);

    let gutter_style = Style::new().dimmed();
    let mut out = String::new();
    for (offset, line) in lines[start - 1..end - 1].iter().enumerate() {
        let number = start + offset;
        let gutter = format!("{number:>width$} │ ");
        push_styled(&mut out, &gutter, gutter_style, color);
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Runs the tree-sitter highlighter and renders styled spans to a string.
///
/// Returns `None` when the language is unsupported or the highlighter errors,
/// which the public functions translate into a plain-text fallback. A
/// `Highlighter` is cheap to construct and not `Sync`, so one is built per call
/// rather than cached.
fn render_spans(language: Language, source: &str, color: bool) -> Option<String> {
    let config = config_for(language)?;
    let mut highlighter = Highlighter::new();

    // Injections are intentionally not resolved: this crate highlights one
    // language per file, so any injected region renders with the outer grammar.
    let events = highlighter
        .highlight(config, source.as_bytes(), None, |_| None)
        .ok()?;

    let mut out = String::with_capacity(source.len());
    let mut stack: Vec<Highlight> = Vec::new();
    for event in events {
        match event.ok()? {
            HighlightEvent::HighlightStart(highlight) => stack.push(highlight),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                let text = source.get(start..end)?;
                let style = stack
                    .last()
                    .and_then(|highlight| HIGHLIGHT_NAMES.get(highlight.0))
                    .map_or_else(Style::new, |name| style_for(name));
                push_styled(&mut out, text, style, color);
            }
        }
    }
    Some(out)
}

/// Number of decimal digits in `n` (at least 1, so `0` has width 1).
const fn decimal_width(mut n: usize) -> usize {
    let mut width = 1;
    while n >= 10 {
        n /= 10;
        width += 1;
    }
    width
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ANSI Control Sequence Introducer; its presence proves color was emitted.
    const CSI: &str = "\u{1b}[";

    #[test]
    fn rust_snippet_is_colored_with_color_true() {
        let source = "fn main() { let x = 42; }\n";
        let out = highlight("main.rs", source, true);
        assert!(out.contains(CSI), "expected ANSI escapes, got: {out:?}");
        // The visible text survives once escapes are stripped of CSI markers.
        assert!(out.contains("main"));
        assert!(out.contains("42"));
    }

    #[test]
    fn rust_snippet_is_plain_with_color_false() {
        let source = "fn main() { let x = 42; }\n";
        let out = highlight("main.rs", source, false);
        assert!(!out.contains(CSI), "expected no ANSI escapes, got: {out:?}");
        assert_eq!(out, source);
    }

    #[test]
    fn bare_extension_resolves() {
        // A caller that only knows the extension, not a path, still highlights.
        let out = highlight("rs", "fn main() {}\n", true);
        assert!(out.contains(CSI), "expected ANSI escapes, got: {out:?}");
    }

    #[test]
    fn lock_file_resolves_by_name() {
        // `uv.lock` has no informative extension; the filename table resolves it
        // to TOML so its snippets highlight instead of rendering plain.
        let source = "[[package]]\nname = \"x\"\nversion = \"0.1.0\"\n";
        let out = highlight("uv.lock", source, true);
        assert!(out.contains(CSI), "expected ANSI escapes, got: {out:?}");
    }

    #[test]
    fn unknown_extension_falls_back_to_plain_text() {
        let source = "<<< not a known language >>>\n";
        let colored = highlight("mystery.zzz", source, true);
        let plain = highlight("mystery.zzz", source, false);
        assert_eq!(colored, source);
        assert_eq!(plain, source);
        assert!(!colored.contains(CSI));
    }

    #[test]
    fn no_extension_falls_back_to_plain_text() {
        let source = "anything at all\n";
        assert_eq!(highlight("LICENSE", source, true), source);
    }

    #[test]
    fn highlight_lines_emits_one_based_gutter() {
        let source = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let out = highlight_lines("x.rs", source, 2, 2, false);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("2 │ "), "got: {:?}", lines[0]);
        assert!(lines[1].starts_with("3 │ "), "got: {:?}", lines[1]);
        assert!(lines[0].contains("fn b()"));
        assert!(lines[1].contains("fn c()"));
    }

    #[test]
    fn highlight_lines_includes_first_line() {
        // A snippet starting at line 1 must show line 1, the regression that the
        // chunk off-by-one masked by dropping it.
        let source = "fn first() {}\nfn second() {}\n";
        let out = highlight_lines("x.rs", source, 1, 2, false);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("1 │ "), "got: {:?}", lines[0]);
        assert!(lines[0].contains("fn first()"));
    }

    #[test]
    fn single_line_file_highlights() {
        // A one-line file with a one-line window renders that line highlighted,
        // not empty (which would trigger the caller's plain-text fallback).
        let source = "SELECT id FROM users;\n";
        let out = highlight_lines("q.sql", source, 1, 1, true);
        assert!(out.contains(CSI), "expected ANSI escapes, got: {out:?}");
        assert!(out.contains("SELECT"));
    }

    #[test]
    fn highlight_lines_color_carries_escapes() {
        let source = "fn a() {}\nfn b() {}\n";
        let out = highlight_lines("x.rs", source, 1, 2, true);
        assert!(out.contains(CSI), "expected ANSI escapes, got: {out:?}");
    }

    #[test]
    fn highlight_lines_plain_has_no_escapes() {
        let source = "fn a() {}\nfn b() {}\n";
        let out = highlight_lines("x.rs", source, 1, 2, false);
        assert!(!out.contains(CSI), "expected no ANSI escapes, got: {out:?}");
    }

    #[test]
    fn highlight_lines_gutter_width_tracks_largest_number() {
        // 12 lines so line 10+ needs two-digit gutters; numbers right-align.
        let mut source = String::new();
        for n in 1..=12 {
            let _ = writeln!(source, "line{n}");
        }
        let out = highlight_lines("x.txt", &source, 9, 3, false);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with(" 9 │ "), "got: {:?}", lines[0]);
        assert!(lines[1].starts_with("10 │ "), "got: {:?}", lines[1]);
        assert!(lines[2].starts_with("11 │ "), "got: {:?}", lines[2]);
    }

    #[test]
    fn highlight_lines_clamps_past_end_of_file() {
        let source = "one\ntwo\n";
        // Request more lines than exist starting at line 2.
        let out = highlight_lines("x.txt", source, 2, 10, false);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("2 │ two"));
    }

    #[test]
    fn highlight_lines_start_past_end_is_empty() {
        let source = "one\ntwo\n";
        assert_eq!(highlight_lines("x.txt", source, 99, 3, false), "");
    }

    #[test]
    fn unknown_language_lines_still_get_gutter() {
        let source = "alpha\nbeta\n";
        let out = highlight_lines("notes.zzz", source, 1, 2, true);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("alpha"));
        // No language means no code color, but the dimmed gutter is still ANSI.
        assert!(out.contains(CSI));
    }

    #[test]
    fn every_grammar_language_builds_a_config() {
        // Every language this crate claims a grammar for must compile its query.
        // Detection-only languages (no grammar) are exempt and render plain.
        for &language in Language::ALL {
            if grammar_query(language).is_some() {
                assert!(
                    config_for(language).is_some(),
                    "{} config failed to build",
                    language.name()
                );
            }
        }
    }

    #[test]
    fn every_grammar_language_highlights_a_sample() {
        // One representative file per grammar proves the query compiles and
        // produces at least one themed span end to end. The samples are
        // deliberately non-trivial: a grammar's highlights query only tags the
        // constructs it names, and a bare `int f() { return 1; }` can tag
        // nothing this theme styles (the C/C++ query, for one, leaves primitive
        // types, `return`, and integer literals unstyled), so each sample
        // includes a keyword, function, string, or member the query is known to
        // capture.
        let cases = [
            (
                "a.rs",
                "fn needle() {\n  let value = 42;\n  println!(\"x\");\n}\n",
            ),
            ("a.py", "def needle():\n    value = 42\n    return \"x\"\n"),
            (
                "a.js",
                "function needle() {\n  const value = 42;\n  return value;\n}\n",
            ),
            (
                "a.ts",
                "function needle(): number {\n  const value: number = 42;\n  return value;\n}\n",
            ),
            (
                "a.tsx",
                "const Needle = () => {\n  const value = 42;\n  return <div className=\"x\">{value}</div>;\n};\n",
            ),
            (
                "a.go",
                "package main\nfunc needle() int {\n  return 42\n}\n",
            ),
            (
                "a.c",
                "int needle(void) {\n  int value = 42;\n  return value;\n}\n",
            ),
            (
                "a.cpp",
                "int needle() {\n  auto value = 42;\n  return value;\n}\n",
            ),
            ("a.cs", "class Needle {\n  int Value() { return 42; }\n}\n"),
            (
                "a.java",
                "class Needle {\n  int value() { return 42; }\n}\n",
            ),
            ("a.scala", "object Needle {\n  def value: Int = 42\n}\n"),
            (
                "a.swift",
                "func needle() -> Int {\n  let value = 42\n  return value\n}\n",
            ),
            ("a.rb", "def needle\n  value = 42\n  \"x\"\nend\n"),
            (
                "a.php",
                "<?php\nfunction needle() {\n  $value = 42;\n  return $value;\n}\n",
            ),
            (
                "a.lua",
                "local function needle()\n  local value = 42\n  return value\nend\n",
            ),
            ("a.hs", "needle :: Int\nneedle = 42\nvalue = needle\n"),
            ("a.ex", "def needle do\n  value = 42\n  value\nend\n"),
            ("a.ml", "let needle =\n  let value = 42 in\n  value\n"),
            (
                "a.html",
                "<div class=\"needle\">\n  <span>value 42</span>\n</div>\n",
            ),
            ("a.css", ".needle {\n  color: red;\n  width: 42px;\n}\n"),
            (
                "a.json",
                "{\n  \"needle\": \"value\",\n  \"count\": 42\n}\n",
            ),
            ("a.toml", "[needle]\nvalue = \"x\"\ncount = 42\n"),
            ("a.yaml", "needle: value\ncount: 42\nlist:\n  - one\n"),
            (
                "a.sql",
                "SELECT needle, value FROM users WHERE count = 42;\n",
            ),
            (
                "a.nix",
                "{ pkgs }: {\n  needle = pkgs.value;\n  count = 42;\n}\n",
            ),
            ("a.sh", "needle() {\n  local value=42\n  echo \"x\"\n}\n"),
            ("a.md", "# Needle value\n\nSome text with count 42.\n"),
        ];
        for (path, source) in cases {
            let out = highlight(path, source, true);
            assert!(
                out.contains(CSI),
                "{path}: expected ANSI escapes, got {out:?}"
            );
        }
    }

    #[test]
    fn dotted_capture_falls_back_to_prefix_style() {
        // `function.method` has its own style; an unmapped dotted name resolves
        // to its prefix.
        assert_ne!(style_for("function.method"), Style::new());
        assert_eq!(style_for("function.weird.nested"), style_for("function"));
        assert_eq!(style_for("totally.unknown"), Style::new());
    }
}
