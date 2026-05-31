use std::path::Path;

use crate::{profiles, types::Profile};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    JavaScript,
    TypeScript,
    TypeScriptTsx,
    Python,
    Go,

    Java,
    Kotlin,
    Scala,

    C,
    Cpp,

    CSharp,

    Swift,

    Ruby,
    Php,
    Bash,
    Lua,

    Haskell,
    Elixir,
    OCaml,

    Html,
    Css,
    Svelte,

    Json,
    Toml,
    Yaml,

    Markdown,

    Dockerfile,

    Nix,
}

impl Lang {
    #[must_use]
    pub fn to_tree_sitter(self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::TypeScriptTsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),

            Self::Java => tree_sitter_java::LANGUAGE.into(),
            Self::Kotlin => tree_sitter_kotlin_ng::LANGUAGE.into(),
            Self::Scala => tree_sitter_scala::LANGUAGE.into(),

            Self::C => tree_sitter_c::LANGUAGE.into(),
            Self::Cpp => tree_sitter_cpp::LANGUAGE.into(),

            Self::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),

            Self::Swift => tree_sitter_swift::LANGUAGE.into(),

            Self::Ruby => tree_sitter_ruby::LANGUAGE.into(),
            Self::Php => tree_sitter_php::LANGUAGE_PHP.into(),
            Self::Bash => tree_sitter_bash::LANGUAGE.into(),
            Self::Lua => tree_sitter_lua::LANGUAGE.into(),

            Self::Haskell => tree_sitter_haskell::LANGUAGE.into(),
            Self::Elixir => tree_sitter_elixir::LANGUAGE.into(),
            Self::OCaml => tree_sitter_ocaml::LANGUAGE_OCAML.into(),

            Self::Html => tree_sitter_html::LANGUAGE.into(),
            Self::Css => tree_sitter_css::LANGUAGE.into(),
            Self::Svelte => tree_sitter_svelte_ng::LANGUAGE.into(),

            Self::Json => tree_sitter_json::LANGUAGE.into(),
            Self::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Self::Yaml => tree_sitter_yaml::LANGUAGE.into(),

            Self::Markdown => tree_sitter_md::LANGUAGE.into(),

            Self::Dockerfile => tree_sitter_dockerfile_updated::language(),

            Self::Nix => tree_sitter_nix::LANGUAGE.into(),
        }
    }

    #[must_use]
    pub fn profile(self) -> &'static Profile {
        profiles::get(self)
    }

    #[must_use]
    pub fn all() -> &'static [Self] {
        &[
            Self::Rust,
            Self::JavaScript,
            Self::TypeScript,
            Self::TypeScriptTsx,
            Self::Python,
            Self::Go,
            Self::Java,
            Self::Kotlin,
            Self::Scala,
            Self::C,
            Self::Cpp,
            Self::CSharp,
            Self::Swift,
            Self::Ruby,
            Self::Php,
            Self::Bash,
            Self::Lua,
            Self::Haskell,
            Self::Elixir,
            Self::OCaml,
            Self::Html,
            Self::Css,
            Self::Svelte,
            Self::Json,
            Self::Toml,
            Self::Yaml,
            Self::Markdown,
            Self::Dockerfile,
            Self::Nix,
        ]
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        self.profile().name
    }
}

#[must_use]
pub fn detect(path: &Path) -> Option<Lang> {
    if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
        for lang in Lang::all() {
            if lang.profile().file_names.contains(&file_name) {
                return Some(*lang);
            }
        }
    }

    let ext = path.extension()?.to_str()?;
    for lang in Lang::all() {
        if lang.profile().extensions.contains(&ext) {
            return Some(*lang);
        }
    }

    None
}

#[must_use]
pub fn detect_from_extension(ext: &str) -> Option<Lang> {
    let ext = ext.strip_prefix('.').unwrap_or(ext);
    for lang in Lang::all() {
        if lang.profile().extensions.contains(&ext) {
            return Some(*lang);
        }
    }
    None
}
