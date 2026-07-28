//! Maps a file path, name, or extension to the source [`Language`] it holds.
//!
//! This crate owns one job: language identification from a path. It carries no
//! grammar, parser, or highlighting dependencies, so a consumer that only needs
//! "what language is this file" (a chunker, a tokenizer, a search ranker) can
//! depend on it without pulling in the tree-sitter grammar closure that a
//! highlighter needs. The sibling `code-highlight` crate layers the
//! grammar-to-query mapping on top of this enum.
//!
//! Resolution prefers a recognized full filename over the extension, so
//! extension-less or misleading-extension files such as `Cargo.lock` (TOML) and
//! `Gemfile` (Ruby) resolve correctly. An unrecognized path yields `None`, which
//! callers treat as "unknown language" (plain text, no special handling).

use std::path::Path;

/// A source language identified from a file path.
///
/// The variant set is the curated list shared with the highlighter; it is
/// `#[non_exhaustive]` so adding a language is not a breaking change for
/// downstream `match`es. [`Language::from_path`] is the usual entry point.
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
    /// TOML (`.toml`, plus `Cargo.lock`, `uv.lock`, `poetry.lock`).
    Toml,
    /// YAML (`.yaml`, `.yml`).
    Yaml,
    /// SQL (`.sql`).
    Sql,
    /// Nix (`.nix`).
    Nix,
    /// Bash and POSIX shell (`.sh`, `.bash`, `.zsh`).
    Bash,
    /// Markdown (`.md`, `.markdown`, `.mdown`, `.mkd`).
    Markdown,
}

impl Language {
    /// Every known language, the single source of truth for the set. Downstream
    /// caches and tests iterate this slice so adding a variant only needs one
    /// edit here.
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
    /// to Ruby and `uv.lock` to TOML), then the extension is tried. Returns
    /// `None` when neither matches; callers treat `None` as an unknown language.
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

    /// Resolves a language from a full file name, covering files whose extension
    /// is absent or does not name the format they hold.
    ///
    /// Lock files are the common case: `Cargo.lock`, `uv.lock`, and
    /// `poetry.lock` are TOML, while `flake.lock` and `deno.lock` are JSON, none
    /// of which is recoverable from the `.lock` extension alone. The match is
    /// case-sensitive because these names are conventionally cased. Returns
    /// `None` otherwise.
    #[must_use]
    pub fn from_file_name(name: &str) -> Option<Self> {
        Some(match name {
            "Gemfile" | "Rakefile" | "Guardfile" | "Capfile" => Self::Ruby,
            "mix.exs" => Self::Elixir,
            "BUILD" | "BUILD.bazel" => Self::Python,
            ".bashrc" | ".bash_profile" | ".zshrc" | ".profile" => Self::Bash,
            "Cargo.lock" | "uv.lock" | "poetry.lock" | "Pipfile" => Self::Toml,
            "flake.lock" | "deno.lock" => Self::Json,
            _ => return None,
        })
    }

    /// The lowercase identifier for this language, suitable as a cache key, an
    /// injection name, or a metadata tag.
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // A recognized filename overrides the extension match path: `mix.exs`
        // resolves to Elixir via the filename table.
        assert_eq!(
            Language::from_path(Path::new("mix.exs")),
            Some(Language::Elixir)
        );
    }

    #[test]
    fn lock_files_resolve_to_their_real_format() {
        // The `.lock` extension is uninformative; the filename names the format.
        for toml in ["Cargo.lock", "uv.lock", "poetry.lock", "Pipfile"] {
            assert_eq!(
                Language::from_path(Path::new(toml)),
                Some(Language::Toml),
                "{toml} should resolve to TOML"
            );
        }
        for json in ["flake.lock", "deno.lock"] {
            assert_eq!(
                Language::from_path(Path::new(json)),
                Some(Language::Json),
                "{json} should resolve to JSON"
            );
        }
        // A lock file whose extension already names its format keeps resolving by
        // extension, and an unknown lock file stays unknown rather than guessing.
        assert_eq!(
            Language::from_path(Path::new("pnpm-lock.yaml")),
            Some(Language::Yaml)
        );
        assert_eq!(Language::from_path(Path::new("yarn.lock")), None);
    }

    #[test]
    fn nested_lock_path_resolves_by_file_name() {
        assert_eq!(
            Language::from_path(Path::new("services/api/uv.lock")),
            Some(Language::Toml)
        );
    }

    #[test]
    fn unknown_paths_are_none() {
        assert_eq!(Language::from_path(Path::new("LICENSE")), None);
        assert_eq!(Language::from_path(Path::new("image.png")), None);
        assert_eq!(Language::from_extension("zzz"), None);
    }

    #[test]
    fn names_are_unique_and_lowercase() {
        let mut names: Vec<&str> = Language::ALL.iter().map(|l| l.name()).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "language names must be unique");
        assert!(
            Language::ALL
                .iter()
                .all(|l| l.name() == l.name().to_ascii_lowercase()),
            "language names must be lowercase"
        );
    }
}
