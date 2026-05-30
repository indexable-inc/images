//! Tree-sitter syntax highlighting for source files, rendered as ANSI text.
//!
//! The crate owns one job: turn a source string plus a language hint (a file
//! path or extension) into colored terminal output. It wraps the official
//! [`tree_sitter_highlight`] crate and a curated set of `tree-sitter-<lang>`
//! grammars, maps the standard highlight capture names to a small
//! [`anstyle`]-based theme, and renders ANSI escapes when the caller asks for
//! color.
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
use serde::Deserialize;
use tree_sitter_highlight::{Highlight, HighlightConfiguration, HighlightEvent, Highlighter};

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

/// A supported source language. Each variant owns a grammar and a highlights
/// query; [`Language::from_path`] maps a file path to a variant by extension.
///
/// The set mirrors the curated grammar list in the ix repo's `ast-merge-langs`
/// crate (extensions and filenames included), minus the few grammars whose
/// published Rust bindings do not export a usable highlights query (Dockerfile,
/// Svelte). All variants resolve through the maintained `tree-sitter-<lang>`
/// crates; an extension this enum does not cover renders as plain text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Language {
    /// Rust (`.rs`).
    Rust,
    /// Python (`.py`, `.pyi`, `.bzl`, `.bazel`).
    Python,
    /// JavaScript (`.js`, `.mjs`, `.cjs`, `.jsx`).
    JavaScript,
    /// TypeScript (`.ts`, `.mts`, `.cts`).
    TypeScript,
    /// TSX (`.tsx`).
    Tsx,
    /// Go (`.go`).
    Go,
    /// C (`.c`, `.h`).
    C,
    /// C++ (`.cpp`, `.cc`, `.cxx`, `.hpp`, `.hh`, `.hxx`, `.h++`, `.c++`).
    Cpp,
    /// C# (`.cs`).
    CSharp,
    /// Java (`.java`).
    Java,
    /// Scala (`.scala`, `.sc`).
    Scala,
    /// Swift (`.swift`).
    Swift,
    /// Ruby (`.rb`, `.rake`, `.gemspec`).
    Ruby,
    /// PHP (`.php`, `.phtml`, `.php3`-`.php7`, `.phps`).
    Php,
    /// Lua (`.lua`).
    Lua,
    /// Haskell (`.hs`, `.lhs`).
    Haskell,
    /// Elixir (`.ex`, `.exs`).
    Elixir,
    /// OCaml (`.ml`, `.mli`).
    OCaml,
    /// HTML (`.html`, `.htm`, `.xhtml`).
    Html,
    /// CSS (`.css`).
    Css,
    /// JSON (`.json`, `.jsonc`).
    Json,
    /// TOML (`.toml`).
    Toml,
    /// YAML (`.yaml`, `.yml`).
    Yaml,
    /// SQL (`.sql`).
    Sql,
    /// Nix (`.nix`).
    Nix,
    /// Bash and POSIX shell (`.sh`, `.bash`, `.zsh`).
    Bash,
    /// Markdown block structure (`.md`, `.markdown`, `.mdown`, `.mkd`).
    Markdown,
}

impl Language {
    /// Every supported language, the single source of truth for the set. The
    /// config cache and tests iterate this slice so adding a variant only needs
    /// one edit here plus its `build_config` arm.
    pub const ALL: &'static [Self] = &[
        Self::Rust,
        Self::Python,
        Self::JavaScript,
        Self::TypeScript,
        Self::Tsx,
        Self::Go,
        Self::C,
        Self::Cpp,
        Self::CSharp,
        Self::Java,
        Self::Scala,
        Self::Swift,
        Self::Ruby,
        Self::Php,
        Self::Lua,
        Self::Haskell,
        Self::Elixir,
        Self::OCaml,
        Self::Html,
        Self::Css,
        Self::Json,
        Self::Toml,
        Self::Yaml,
        Self::Sql,
        Self::Nix,
        Self::Bash,
        Self::Markdown,
    ];

    /// Resolves a language from a file path.
    ///
    /// A recognized full filename wins over the extension (so `Gemfile` resolves
    /// to Ruby and `Cargo.toml` to TOML), then the extension is tried. Returns
    /// `None` when neither matches; callers treat `None` as "render as plain
    /// text".
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        if let Some(language) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(Self::from_file_name)
        {
            return Some(language);
        }
        path.extension()
            .and_then(|ext| ext.to_str())
            .and_then(Self::from_extension)
    }

    /// Resolves a language from a bare extension (without the leading dot).
    ///
    /// The match is case-insensitive. Returns `None` for unrecognized
    /// extensions.
    #[must_use]
    pub fn from_extension(ext: &str) -> Option<Self> {
        let ext = ext.to_ascii_lowercase();
        Some(match ext.as_str() {
            "rs" => Self::Rust,
            "py" | "pyi" | "bzl" | "bazel" => Self::Python,
            "js" | "mjs" | "cjs" | "jsx" => Self::JavaScript,
            "ts" | "mts" | "cts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "go" => Self::Go,
            "c" | "h" => Self::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "h++" | "c++" => Self::Cpp,
            "cs" => Self::CSharp,
            "java" => Self::Java,
            "scala" | "sc" => Self::Scala,
            "swift" => Self::Swift,
            "rb" | "rake" | "gemspec" => Self::Ruby,
            "php" | "phtml" | "php3" | "php4" | "php5" | "php7" | "phps" => Self::Php,
            "lua" => Self::Lua,
            "hs" | "lhs" => Self::Haskell,
            "ex" | "exs" => Self::Elixir,
            "ml" | "mli" => Self::OCaml,
            "html" | "htm" | "xhtml" => Self::Html,
            "css" => Self::Css,
            "json" | "jsonc" => Self::Json,
            "toml" => Self::Toml,
            "yaml" | "yml" => Self::Yaml,
            "sql" => Self::Sql,
            "nix" => Self::Nix,
            "sh" | "bash" | "zsh" => Self::Bash,
            "md" | "markdown" | "mdown" | "mkd" => Self::Markdown,
            _ => return None,
        })
    }

    /// Resolves a language from a full file name with no significant extension,
    /// such as `Gemfile` or `Rakefile`. The match is case-sensitive because
    /// these names are conventionally cased. Returns `None` otherwise.
    #[must_use]
    pub fn from_file_name(name: &str) -> Option<Self> {
        Some(match name {
            "Gemfile" | "Rakefile" | "Guardfile" | "Capfile" => Self::Ruby,
            "mix.exs" => Self::Elixir,
            "BUILD" | "BUILD.bazel" => Self::Python,
            ".bashrc" | ".bash_profile" | ".zshrc" | ".profile" => Self::Bash,
            _ => return None,
        })
    }

    /// The lowercase identifier for this language, suitable as an injection name
    /// or a cache key.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::Go => "go",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::CSharp => "c_sharp",
            Self::Java => "java",
            Self::Scala => "scala",
            Self::Swift => "swift",
            Self::Ruby => "ruby",
            Self::Php => "php",
            Self::Lua => "lua",
            Self::Haskell => "haskell",
            Self::Elixir => "elixir",
            Self::OCaml => "ocaml",
            Self::Html => "html",
            Self::Css => "css",
            Self::Json => "json",
            Self::Toml => "toml",
            Self::Yaml => "yaml",
            Self::Sql => "sql",
            Self::Nix => "nix",
            Self::Bash => "bash",
            Self::Markdown => "markdown",
        }
    }

    /// Builds a [`HighlightConfiguration`] for this language.
    ///
    /// Each grammar exports its highlights query under a slightly different
    /// constant name (`HIGHLIGHTS_QUERY`, `HIGHLIGHT_QUERY`, or the block query
    /// for Markdown), so the per-language arm names the right one. TypeScript and
    /// TSX inherit the JavaScript highlights query: the TypeScript grammar's own
    /// query only adds type-level rules and expects the ECMAScript rules to be
    /// present, so the JS query is prepended.
    ///
    /// The injection and locals queries are left empty: this crate highlights
    /// one language per file and resolves no injections, so the queries would do
    /// nothing and only add per-grammar constant-name fragility.
    #[allow(
        clippy::too_many_lines,
        reason = "flat one-arm-per-language dispatch table; splitting it would hide the grammar-to-query mapping"
    )]
    fn build_config(self) -> Result<HighlightConfiguration, tree_sitter::QueryError> {
        let (language, highlights) = match self {
            Self::Rust => (
                tree_sitter_rust::LANGUAGE.into(),
                tree_sitter_rust::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::Python => (
                tree_sitter_python::LANGUAGE.into(),
                tree_sitter_python::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::JavaScript => (
                tree_sitter_javascript::LANGUAGE.into(),
                tree_sitter_javascript::HIGHLIGHT_QUERY.to_owned(),
            ),
            Self::TypeScript => (
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                format!(
                    "{}\n{}",
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                    tree_sitter_typescript::HIGHLIGHTS_QUERY
                ),
            ),
            Self::Tsx => (
                tree_sitter_typescript::LANGUAGE_TSX.into(),
                format!(
                    "{}\n{}",
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                    tree_sitter_typescript::HIGHLIGHTS_QUERY
                ),
            ),
            Self::Go => (
                tree_sitter_go::LANGUAGE.into(),
                tree_sitter_go::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::C => (
                tree_sitter_c::LANGUAGE.into(),
                tree_sitter_c::HIGHLIGHT_QUERY.to_owned(),
            ),
            Self::Cpp => (
                tree_sitter_cpp::LANGUAGE.into(),
                tree_sitter_cpp::HIGHLIGHT_QUERY.to_owned(),
            ),
            Self::CSharp => (
                tree_sitter_c_sharp::LANGUAGE.into(),
                tree_sitter_c_sharp::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::Java => (
                tree_sitter_java::LANGUAGE.into(),
                tree_sitter_java::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::Scala => (
                tree_sitter_scala::LANGUAGE.into(),
                tree_sitter_scala::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::Swift => (
                tree_sitter_swift::LANGUAGE.into(),
                tree_sitter_swift::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::Ruby => (
                tree_sitter_ruby::LANGUAGE.into(),
                tree_sitter_ruby::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::Php => (
                tree_sitter_php::LANGUAGE_PHP.into(),
                tree_sitter_php::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::Lua => (
                tree_sitter_lua::LANGUAGE.into(),
                tree_sitter_lua::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::Haskell => (
                tree_sitter_haskell::LANGUAGE.into(),
                tree_sitter_haskell::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::Elixir => (
                tree_sitter_elixir::LANGUAGE.into(),
                tree_sitter_elixir::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::OCaml => (
                tree_sitter_ocaml::LANGUAGE_OCAML.into(),
                tree_sitter_ocaml::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::Html => (
                tree_sitter_html::LANGUAGE.into(),
                tree_sitter_html::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::Css => (
                tree_sitter_css::LANGUAGE.into(),
                tree_sitter_css::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::Json => (
                tree_sitter_json::LANGUAGE.into(),
                tree_sitter_json::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::Toml => (
                tree_sitter_toml_ng::LANGUAGE.into(),
                tree_sitter_toml_ng::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::Yaml => (
                tree_sitter_yaml::LANGUAGE.into(),
                tree_sitter_yaml::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::Sql => (
                tree_sitter_sequel::LANGUAGE.into(),
                tree_sitter_sequel::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::Nix => (
                tree_sitter_nix::LANGUAGE.into(),
                tree_sitter_nix::HIGHLIGHTS_QUERY.to_owned(),
            ),
            Self::Bash => (
                tree_sitter_bash::LANGUAGE.into(),
                tree_sitter_bash::HIGHLIGHT_QUERY.to_owned(),
            ),
            Self::Markdown => (
                tree_sitter_md::LANGUAGE.into(),
                tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.to_owned(),
            ),
        };

        let mut config = HighlightConfiguration::new(language, self.name(), &highlights, "", "")?;
        config.configure(HIGHLIGHT_NAMES);
        Ok(config)
    }
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
        .map(|&language| (language, language.build_config().ok()))
        .collect()
});

/// Returns the cached config for a language, or `None` if it is unsupported or
/// failed to build.
fn config_for(language: Language) -> Option<&'static HighlightConfiguration> {
    CONFIGS.get(&language).and_then(Option::as_ref)
}

/// A color variant of the islands theme.
///
/// The renderer takes one per call so a caller can match the terminal
/// background; the slot colors come from `islands-theme.json`, the single
/// source of truth this crate shares with the base-profile Neovim colorscheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    /// Islands Dark, the default for a dark or unknown background.
    #[default]
    Dark,
    /// Islands Light, for a light background.
    Light,
}

/// The islands palette: one named-slot color table per variant, deserialized
/// from the embedded JSON. The provenance `_comment` field is ignored.
#[derive(Debug, Deserialize)]
struct IslandsTheme {
    dark: HashMap<String, String>,
    light: HashMap<String, String>,
}

/// The palette, parsed once from the embedded source of truth. The JSON ships in
/// the crate, so a parse failure is a build-time-authored bug and panicking is
/// the right signal rather than a silent fallback.
static THEME: LazyLock<IslandsTheme> = LazyLock::new(|| {
    serde_json::from_str(include_str!("islands-theme.json"))
        .expect("islands-theme.json is valid JSON with `dark` and `light` tables")
});

/// Looks up a palette slot's color for a variant, returning `None` when the slot
/// is absent or its value is not a `#RRGGBB` hex string.
fn slot_color(theme: Theme, slot: &str) -> Option<RgbColor> {
    let table = match theme {
        Theme::Dark => &THEME.dark,
        Theme::Light => &THEME.light,
    };
    parse_hex(table.get(slot)?)
}

/// Parses `#RRGGBB`. An optional 8-digit alpha suffix is ignored: terminals have
/// no alpha, so the few UI slots that carry one degrade to their opaque color.
fn parse_hex(hex: &str) -> Option<RgbColor> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() < 6 {
        return None;
    }
    let byte = |range: std::ops::Range<usize>| u8::from_str_radix(digits.get(range)?, 16).ok();
    Some(RgbColor(byte(0..2)?, byte(2..4)?, byte(4..6)?))
}

/// Maps a tree-sitter capture name to an islands palette slot and whether it
/// renders italic. The mapping mirrors the `@capture` wiring in the Neovim
/// colorscheme so the terminal and the editor color the same constructs alike;
/// the colors themselves live only in `islands-theme.json`. Returns `None` for
/// captures the theme leaves at the terminal default.
fn slot_for_name(name: &str) -> Option<(&'static str, bool)> {
    Some(match name {
        "keyword" | "constant.builtin" | "type.builtin" | "function.macro" | "label" => {
            ("keyword", false)
        }
        "variable.builtin" => ("this_self", false),
        "function" | "function.builtin" | "function.method" | "constructor" => ("func", false),
        "attribute" => ("decorator", false),
        "type" => ("type", false),
        "module" | "embedded" => ("fg", false),
        "string" | "string.special" => ("string", false),
        "escape" | "string.escape" | "punctuation.special" => ("string_escape", false),
        "number" => ("number", false),
        "property" | "variable.member" => ("property", false),
        "tag" => ("tag", false),
        "variable" => ("variable", false),
        "variable.parameter" => ("parameter", false),
        "operator" => ("operator", false),
        "punctuation" | "punctuation.bracket" | "punctuation.delimiter" => ("punctuation", false),
        "comment" => ("comment", true),
        "comment.documentation" => ("doc_comment", true),
        "constant" => ("constant", true),
        _ => return None,
    })
}

/// Resolves the [`Style`] for a capture name in a theme variant, falling back
/// from the most specific dotted name to its prefixes (`function.method` to
/// `function`) and then to the terminal default.
fn style_for(name: &str, theme: Theme) -> Style {
    let mut current = name;
    loop {
        if let Some((slot, italic)) = slot_for_name(current) {
            let style = slot_color(theme, slot)
                .map_or_else(Style::new, |rgb| Style::new().fg_color(Some(Color::Rgb(rgb))));
            return if italic { style.italic() } else { style };
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

/// Highlights a full source string and returns it as a single rendered block.
///
/// `path_or_lang` is the source path or bare extension used to pick a grammar;
/// pass the real file path when you have one so the extension resolves. `theme`
/// selects the islands color variant. When `color` is `true` the output carries
/// ANSI SGR escapes; when `false` it is the input text unchanged.
///
/// Unsupported languages and any highlighter failure fall back to returning the
/// source verbatim, so this function never errors.
#[must_use]
pub fn highlight(path_or_lang: &str, source: &str, theme: Theme, color: bool) -> String {
    let Some(language) = Language::from_extension(extension_of(path_or_lang)) else {
        return source.to_owned();
    };
    render_spans(language, source, theme, color).unwrap_or_else(|| source.to_owned())
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
    theme: Theme,
    color: bool,
) -> String {
    let language = Language::from_extension(extension_of(path_or_lang));
    let rendered = language
        .and_then(|language| render_spans(language, source, theme, color))
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
fn render_spans(language: Language, source: &str, theme: Theme, color: bool) -> Option<String> {
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
                    .map_or_else(Style::new, |name| style_for(name, theme));
                push_styled(&mut out, text, style, color);
            }
        }
    }
    Some(out)
}

/// Extracts the extension from a path or returns the input when it has no `.`
/// (so a bare language name like `"rust"` is rejected but `"rs"` is not; callers
/// should pass an extension or a path).
fn extension_of(path_or_lang: &str) -> &str {
    Path::new(path_or_lang)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or(path_or_lang)
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
        let out = highlight("main.rs", source, Theme::Dark, true);
        assert!(out.contains(CSI), "expected ANSI escapes, got: {out:?}");
        // The visible text survives once escapes are stripped of CSI markers.
        assert!(out.contains("main"));
        assert!(out.contains("42"));
    }

    #[test]
    fn rust_snippet_is_plain_with_color_false() {
        let source = "fn main() { let x = 42; }\n";
        let out = highlight("main.rs", source, Theme::Dark, false);
        assert!(!out.contains(CSI), "expected no ANSI escapes, got: {out:?}");
        assert_eq!(out, source);
    }

    #[test]
    fn unknown_extension_falls_back_to_plain_text() {
        let source = "<<< not a known language >>>\n";
        let colored = highlight("mystery.zzz", source, Theme::Dark, true);
        let plain = highlight("mystery.zzz", source, Theme::Dark, false);
        assert_eq!(colored, source);
        assert_eq!(plain, source);
        assert!(!colored.contains(CSI));
    }

    #[test]
    fn no_extension_falls_back_to_plain_text() {
        let source = "anything at all\n";
        assert_eq!(highlight("LICENSE", source, Theme::Dark, true), source);
    }

    #[test]
    fn highlight_lines_emits_one_based_gutter() {
        let source = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let out = highlight_lines("x.rs", source, 2, 2, Theme::Dark, false);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("2 │ "), "got: {:?}", lines[0]);
        assert!(lines[1].starts_with("3 │ "), "got: {:?}", lines[1]);
        assert!(lines[0].contains("fn b()"));
        assert!(lines[1].contains("fn c()"));
    }

    #[test]
    fn highlight_lines_color_carries_escapes() {
        let source = "fn a() {}\nfn b() {}\n";
        let out = highlight_lines("x.rs", source, 1, 2, Theme::Dark, true);
        assert!(out.contains(CSI), "expected ANSI escapes, got: {out:?}");
    }

    #[test]
    fn highlight_lines_plain_has_no_escapes() {
        let source = "fn a() {}\nfn b() {}\n";
        let out = highlight_lines("x.rs", source, 1, 2, Theme::Dark, false);
        assert!(!out.contains(CSI), "expected no ANSI escapes, got: {out:?}");
    }

    #[test]
    fn highlight_lines_gutter_width_tracks_largest_number() {
        // 12 lines so line 10+ needs two-digit gutters; numbers right-align.
        let mut source = String::new();
        for n in 1..=12 {
            let _ = writeln!(source, "line{n}");
        }
        let out = highlight_lines("x.txt", &source, 9, 3, Theme::Dark, false);
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
        let out = highlight_lines("x.txt", source, 2, 10, Theme::Dark, false);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("2 │ two"));
    }

    #[test]
    fn highlight_lines_start_past_end_is_empty() {
        let source = "one\ntwo\n";
        assert_eq!(highlight_lines("x.txt", source, 99, 3, Theme::Dark, false), "");
    }

    #[test]
    fn unknown_language_lines_still_get_gutter() {
        let source = "alpha\nbeta\n";
        let out = highlight_lines("notes.zzz", source, 1, 2, Theme::Dark, true);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("alpha"));
        // No language means no code color, but the dimmed gutter is still ANSI.
        assert!(out.contains(CSI));
    }

    #[test]
    fn extension_resolution_is_case_insensitive() {
        assert_eq!(Language::from_extension("RS"), Some(Language::Rust));
        assert_eq!(
            Language::from_path(Path::new("a/b/C.PY")),
            Some(Language::Python)
        );
    }

    #[test]
    fn full_file_name_wins_over_extension() {
        // `Gemfile` has no extension; `Rakefile` likewise resolves to Ruby.
        assert_eq!(
            Language::from_path(Path::new("repo/Gemfile")),
            Some(Language::Ruby)
        );
        assert_eq!(
            Language::from_path(Path::new("Rakefile")),
            Some(Language::Ruby)
        );
        // A recognized filename overrides a misleading extension match path:
        // `mix.exs` resolves to Elixir via the filename table.
        assert_eq!(
            Language::from_path(Path::new("mix.exs")),
            Some(Language::Elixir)
        );
    }

    #[test]
    fn every_supported_language_builds_a_config() {
        for &language in Language::ALL {
            assert!(
                config_for(language).is_some(),
                "{} config failed to build",
                language.name()
            );
        }
    }

    #[test]
    fn a_sample_of_languages_highlight_with_color() {
        // One representative file per grammar family proves the query compiles
        // and produces at least one colored span end to end.
        let cases = [
            ("a.py", "def f():\n    return 1\n"),
            ("a.go", "package main\nfunc main() {}\n"),
            ("a.rb", "def greet\n  puts 'hi'\nend\n"),
            ("a.lua", "local function f() return 1 end\n"),
            ("a.css", "body { color: red; }\n"),
            ("a.yaml", "key: value\nlist:\n  - one\n"),
            ("a.sql", "SELECT id FROM users WHERE id = 1;\n"),
            ("a.nix", "{ pkgs }: pkgs.hello\n"),
            ("a.ts", "const x: number = 1;\n"),
        ];
        for (path, source) in cases {
            let out = highlight(path, source, Theme::Dark, true);
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
        assert_ne!(style_for("function.method", Theme::Dark), Style::new());
        assert_eq!(
            style_for("function.weird.nested", Theme::Dark),
            style_for("function", Theme::Dark)
        );
        assert_eq!(style_for("totally.unknown", Theme::Dark), Style::new());
    }

    #[test]
    fn theme_variants_color_the_same_token_differently() {
        // `fn` is a keyword, which islands renders orange on dark and blue on
        // light. The two outputs must differ, proving the variant flows through
        // to the rendered escapes rather than being ignored.
        let source = "fn main() {}\n";
        let dark = highlight("a.rs", source, Theme::Dark, true);
        let light = highlight("a.rs", source, Theme::Light, true);
        assert!(dark.contains(CSI) && light.contains(CSI));
        assert_ne!(dark, light, "dark and light should color the keyword apart");
    }

    #[test]
    fn parse_hex_ignores_alpha_suffix() {
        // UI slots may carry an 8-digit `#RRGGBBAA`; the terminal has no alpha,
        // so the parse keeps the opaque color and drops the suffix.
        assert_eq!(parse_hex("#11495780"), parse_hex("#114957"));
        assert_eq!(parse_hex("#114957"), Some(RgbColor(0x11, 0x49, 0x57)));
        assert_eq!(parse_hex("nope"), None);
    }

    #[test]
    fn every_mapped_slot_exists_in_both_variants() {
        // Cross-file invariant: every slot the capture map names must exist in
        // both palettes, or a capture would silently render uncolored. Guards
        // against a typo in the Rust map or a missing key in the JSON.
        for &name in HIGHLIGHT_NAMES {
            if let Some((slot, _)) = slot_for_name(name) {
                assert!(
                    slot_color(Theme::Dark, slot).is_some(),
                    "dark palette missing slot {slot:?} for capture {name:?}"
                );
                assert!(
                    slot_color(Theme::Light, slot).is_some(),
                    "light palette missing slot {slot:?} for capture {name:?}"
                );
            }
        }
    }
}
