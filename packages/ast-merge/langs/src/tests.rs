use std::path::PathBuf;

use super::*;

mod validation;

#[test]
fn test_detect_rust() {
    let path = PathBuf::from("src/main.rs");
    assert_eq!(detect(&path), Some(Lang::Rust));
}

#[test]
fn test_detect_javascript() {
    assert_eq!(detect(&PathBuf::from("app.js")), Some(Lang::JavaScript));
    assert_eq!(detect(&PathBuf::from("app.mjs")), Some(Lang::JavaScript));
    assert_eq!(detect(&PathBuf::from("app.cjs")), Some(Lang::JavaScript));
    assert_eq!(detect(&PathBuf::from("app.jsx")), Some(Lang::JavaScript));
}

#[test]
fn test_detect_typescript() {
    assert_eq!(detect(&PathBuf::from("app.ts")), Some(Lang::TypeScript));
    assert_eq!(
        detect(&PathBuf::from("component.tsx")),
        Some(Lang::TypeScriptTsx)
    );
}

#[test]
fn test_detect_python() {
    assert_eq!(detect(&PathBuf::from("script.py")), Some(Lang::Python));
    assert_eq!(detect(&PathBuf::from("types.pyi")), Some(Lang::Python));

    assert_eq!(detect(&PathBuf::from("BUILD")), Some(Lang::Python));
    assert_eq!(detect(&PathBuf::from("BUILD.bazel")), Some(Lang::Python));
    assert_eq!(detect(&PathBuf::from("rules.bzl")), Some(Lang::Python));
}

#[test]
fn test_detect_go() {
    let path = PathBuf::from("main.go");
    assert_eq!(detect(&path), Some(Lang::Go));
}

#[test]
fn test_detect_java() {
    let path = PathBuf::from("Main.java");
    assert_eq!(detect(&path), Some(Lang::Java));
}

#[test]
fn test_detect_kotlin() {
    assert_eq!(detect(&PathBuf::from("App.kt")), Some(Lang::Kotlin));
    assert_eq!(
        detect(&PathBuf::from("build.gradle.kts")),
        Some(Lang::Kotlin)
    );
}

#[test]
fn test_detect_scala() {
    assert_eq!(detect(&PathBuf::from("App.scala")), Some(Lang::Scala));
    assert_eq!(detect(&PathBuf::from("script.sc")), Some(Lang::Scala));
}

#[test]
fn test_detect_c() {
    assert_eq!(detect(&PathBuf::from("main.c")), Some(Lang::C));
    assert_eq!(detect(&PathBuf::from("header.h")), Some(Lang::C));
}

#[test]
fn test_detect_cpp() {
    assert_eq!(detect(&PathBuf::from("main.cpp")), Some(Lang::Cpp));
    assert_eq!(detect(&PathBuf::from("main.cc")), Some(Lang::Cpp));
    assert_eq!(detect(&PathBuf::from("header.hpp")), Some(Lang::Cpp));
}

#[test]
fn test_detect_csharp() {
    let path = PathBuf::from("Program.cs");
    assert_eq!(detect(&path), Some(Lang::CSharp));
}

#[test]
fn test_detect_swift() {
    let path = PathBuf::from("ViewController.swift");
    assert_eq!(detect(&path), Some(Lang::Swift));
}

#[test]
fn test_detect_ruby() {
    assert_eq!(detect(&PathBuf::from("app.rb")), Some(Lang::Ruby));
    assert_eq!(detect(&PathBuf::from("Gemfile")), Some(Lang::Ruby));
    assert_eq!(detect(&PathBuf::from("Rakefile")), Some(Lang::Ruby));
}

#[test]
fn test_detect_php() {
    assert_eq!(detect(&PathBuf::from("index.php")), Some(Lang::Php));
}

#[test]
fn test_detect_bash() {
    assert_eq!(detect(&PathBuf::from("script.sh")), Some(Lang::Bash));
    assert_eq!(detect(&PathBuf::from("script.bash")), Some(Lang::Bash));
    assert_eq!(detect(&PathBuf::from(".bashrc")), Some(Lang::Bash));
}

#[test]
fn test_detect_lua() {
    let path = PathBuf::from("init.lua");
    assert_eq!(detect(&path), Some(Lang::Lua));
}

#[test]
fn test_detect_haskell() {
    assert_eq!(detect(&PathBuf::from("Main.hs")), Some(Lang::Haskell));
    assert_eq!(detect(&PathBuf::from("Lib.lhs")), Some(Lang::Haskell));
}

#[test]
fn test_detect_elixir() {
    assert_eq!(detect(&PathBuf::from("app.ex")), Some(Lang::Elixir));
    assert_eq!(detect(&PathBuf::from("mix.exs")), Some(Lang::Elixir));
}

#[test]
fn test_detect_ocaml() {
    assert_eq!(detect(&PathBuf::from("main.ml")), Some(Lang::OCaml));
    assert_eq!(detect(&PathBuf::from("lib.mli")), Some(Lang::OCaml));
}

#[test]
fn test_detect_html() {
    assert_eq!(detect(&PathBuf::from("index.html")), Some(Lang::Html));
    assert_eq!(detect(&PathBuf::from("page.htm")), Some(Lang::Html));
}

#[test]
fn test_detect_css() {
    let path = PathBuf::from("styles.css");
    assert_eq!(detect(&path), Some(Lang::Css));
}

#[test]
fn test_detect_svelte() {
    assert_eq!(detect(&PathBuf::from("App.svelte")), Some(Lang::Svelte));
    assert_eq!(detect(&PathBuf::from("+page.svelte")), Some(Lang::Svelte));
}

#[test]
fn test_detect_json() {
    let path = PathBuf::from("package.json");
    assert_eq!(detect(&path), Some(Lang::Json));
}

#[test]
fn test_detect_toml() {
    let path = PathBuf::from("Cargo.toml");
    assert_eq!(detect(&path), Some(Lang::Toml));
}

#[test]
fn test_detect_yaml() {
    assert_eq!(detect(&PathBuf::from("config.yaml")), Some(Lang::Yaml));
    assert_eq!(detect(&PathBuf::from("config.yml")), Some(Lang::Yaml));
}

#[test]
fn test_detect_markdown() {
    assert_eq!(detect(&PathBuf::from("README.md")), Some(Lang::Markdown));
    assert_eq!(
        detect(&PathBuf::from("docs.markdown")),
        Some(Lang::Markdown)
    );
}

#[test]
fn test_detect_dockerfile() {
    assert_eq!(detect(&PathBuf::from("Dockerfile")), Some(Lang::Dockerfile));
    assert_eq!(
        detect(&PathBuf::from("Containerfile")),
        Some(Lang::Dockerfile)
    );
}

#[test]
fn test_detect_unknown() {
    let path = PathBuf::from("file.xyz");
    assert_eq!(detect(&path), None);
}

#[test]
fn test_detect_from_extension() {
    assert_eq!(detect_from_extension("rs"), Some(Lang::Rust));
    assert_eq!(detect_from_extension(".rs"), Some(Lang::Rust));
    assert_eq!(detect_from_extension("java"), Some(Lang::Java));
    assert_eq!(detect_from_extension("kt"), Some(Lang::Kotlin));
    assert_eq!(detect_from_extension("swift"), Some(Lang::Swift));
    assert_eq!(detect_from_extension("unknown"), None);
}
