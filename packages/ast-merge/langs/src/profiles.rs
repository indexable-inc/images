use crate::{lang::Lang, types::Profile};

mod compiled;
mod config;
mod functional;
mod jvm;
mod scripting;
mod web;

pub fn get(lang: Lang) -> &'static Profile {
    match lang {
        Lang::Rust => &compiled::RUST,
        Lang::C => &compiled::C,
        Lang::Cpp => &compiled::CPP,
        Lang::CSharp => &compiled::CSHARP,
        Lang::Swift => &compiled::SWIFT,
        Lang::Go => &compiled::GO,

        Lang::Java => &jvm::JAVA,
        Lang::Kotlin => &jvm::KOTLIN,
        Lang::Scala => &jvm::SCALA,

        Lang::JavaScript => &web::JAVASCRIPT,
        Lang::TypeScript => &web::TYPESCRIPT,
        Lang::TypeScriptTsx => &web::TSX,
        Lang::Html => &web::HTML,
        Lang::Css => &web::CSS,
        Lang::Svelte => &web::SVELTE,

        Lang::Python => &scripting::PYTHON,
        Lang::Ruby => &scripting::RUBY,
        Lang::Php => &scripting::PHP,
        Lang::Bash => &scripting::BASH,
        Lang::Lua => &scripting::LUA,

        Lang::Haskell => &functional::HASKELL,
        Lang::Elixir => &functional::ELIXIR,
        Lang::OCaml => &functional::OCAML,

        Lang::Json => &config::JSON,
        Lang::Toml => &config::TOML,
        Lang::Yaml => &config::YAML,
        Lang::Markdown => &config::MARKDOWN,
        Lang::Dockerfile => &config::DOCKERFILE,
        Lang::Nix => &config::NIX,
    }
}
