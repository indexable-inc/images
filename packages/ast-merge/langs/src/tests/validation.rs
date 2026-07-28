use super::super::*;

#[test]
fn test_language_all() {
    let all = Lang::all();

    assert!(
        all.len() >= 28,
        "Expected at least 28 languages, got {}",
        all.len()
    );

    assert!(all.contains(&Lang::Rust));
    assert!(all.contains(&Lang::TypeScript));
    assert!(all.contains(&Lang::Java));
    assert!(all.contains(&Lang::Kotlin));
    assert!(all.contains(&Lang::Swift));
    assert!(all.contains(&Lang::Python));
    assert!(all.contains(&Lang::Go));
    assert!(all.contains(&Lang::CSharp));
    assert!(all.contains(&Lang::Ruby));
    assert!(all.contains(&Lang::Bash));
}

#[test]
fn test_language_name() {
    assert_eq!(Lang::Rust.name(), "Rust");
    assert_eq!(Lang::TypeScript.name(), "TypeScript");
    assert_eq!(Lang::Java.name(), "Java");
    assert_eq!(Lang::Kotlin.name(), "Kotlin");
    assert_eq!(Lang::Swift.name(), "Swift");
    assert_eq!(Lang::CSharp.name(), "C#");
}

#[test]
fn test_tree_sitter_core_languages() {
    assert!(Lang::Rust.to_tree_sitter().node_kind_count() > 0);
    assert!(Lang::JavaScript.to_tree_sitter().node_kind_count() > 0);
    assert!(Lang::TypeScript.to_tree_sitter().node_kind_count() > 0);
    assert!(Lang::TypeScriptTsx.to_tree_sitter().node_kind_count() > 0);
    assert!(Lang::Python.to_tree_sitter().node_kind_count() > 0);
    assert!(Lang::Go.to_tree_sitter().node_kind_count() > 0);
}

#[test]
fn test_tree_sitter_jvm_languages() {
    assert!(Lang::Java.to_tree_sitter().node_kind_count() > 0);
    assert!(Lang::Kotlin.to_tree_sitter().node_kind_count() > 0);
    assert!(Lang::Scala.to_tree_sitter().node_kind_count() > 0);
}

#[test]
fn test_tree_sitter_systems_languages() {
    assert!(Lang::C.to_tree_sitter().node_kind_count() > 0);
    assert!(Lang::Cpp.to_tree_sitter().node_kind_count() > 0);
}

#[test]
fn test_tree_sitter_dotnet_languages() {
    assert!(Lang::CSharp.to_tree_sitter().node_kind_count() > 0);
}

#[test]
fn test_tree_sitter_mobile_languages() {
    assert!(Lang::Swift.to_tree_sitter().node_kind_count() > 0);
}

#[test]
fn test_tree_sitter_scripting_languages() {
    assert!(Lang::Ruby.to_tree_sitter().node_kind_count() > 0);
    assert!(Lang::Php.to_tree_sitter().node_kind_count() > 0);
    assert!(Lang::Bash.to_tree_sitter().node_kind_count() > 0);
    assert!(Lang::Lua.to_tree_sitter().node_kind_count() > 0);
}

#[test]
fn test_tree_sitter_functional_languages() {
    assert!(Lang::Haskell.to_tree_sitter().node_kind_count() > 0);
    assert!(Lang::Elixir.to_tree_sitter().node_kind_count() > 0);
    assert!(Lang::OCaml.to_tree_sitter().node_kind_count() > 0);
}

#[test]
fn test_tree_sitter_web_languages() {
    assert!(Lang::Html.to_tree_sitter().node_kind_count() > 0);
    assert!(Lang::Css.to_tree_sitter().node_kind_count() > 0);
    assert!(Lang::Svelte.to_tree_sitter().node_kind_count() > 0);
}

#[test]
fn test_tree_sitter_data_formats() {
    assert!(Lang::Json.to_tree_sitter().node_kind_count() > 0);
    assert!(Lang::Toml.to_tree_sitter().node_kind_count() > 0);
    assert!(Lang::Yaml.to_tree_sitter().node_kind_count() > 0);
}

#[test]
fn test_tree_sitter_documentation() {
    assert!(Lang::Markdown.to_tree_sitter().node_kind_count() > 0);
}

#[test]
fn test_tree_sitter_devops() {
    assert!(Lang::Dockerfile.to_tree_sitter().node_kind_count() > 0);
}

#[test]
fn test_rust_profile() {
    let profile = Lang::Rust.profile();
    assert!(profile.is_atomic("use_declaration"));
    assert!(!profile.is_atomic("function_item"));
    assert!(profile.is_commutative("declaration_list"));
    assert!(profile.is_comment("line_comment"));
}

#[test]
fn test_typescript_profile() {
    let profile = Lang::TypeScript.profile();
    assert!(profile.is_atomic("import_statement"));
    assert!(profile.is_commutative("interface_body"));
}

#[test]
fn test_python_profile() {
    let profile = Lang::Python.profile();
    assert!(profile.is_atomic("import_statement"));
    assert!(profile.is_commutative("dictionary"));
}

#[test]
fn test_java_profile() {
    let profile = Lang::Java.profile();
    assert!(profile.is_atomic("import_declaration"));
    assert!(profile.is_commutative("class_body"));
    assert!(profile.is_comment("line_comment"));
}

#[test]
fn test_kotlin_profile() {
    let profile = Lang::Kotlin.profile();
    assert!(profile.is_atomic("import_header"));
    assert!(profile.is_commutative("class_body"));
}

#[test]
fn test_swift_profile() {
    let profile = Lang::Swift.profile();
    assert!(profile.is_atomic("import_declaration"));
    assert!(profile.is_commutative("class_body"));
}

#[test]
fn test_csharp_profile() {
    let profile = Lang::CSharp.profile();
    assert!(profile.is_atomic("using_directive"));
    assert!(profile.is_commutative("class_declaration"));
}

#[test]
fn test_bash_profile() {
    let profile = Lang::Bash.profile();
    assert!(profile.is_atomic("command"));
    assert!(profile.is_commutative("case_statement"));
    assert!(profile.is_comment("comment"));
}

#[test]
fn test_ruby_profile() {
    let profile = Lang::Ruby.profile();
    assert!(profile.is_atomic("require"));
    assert!(profile.is_commutative("hash"));
}

#[test]
fn test_c_profile() {
    let profile = Lang::C.profile();
    assert!(profile.is_atomic("preproc_include"));
    assert!(profile.is_commutative("field_declaration_list"));
}

#[test]
fn test_cpp_profile() {
    let profile = Lang::Cpp.profile();
    assert!(profile.is_atomic("preproc_include"));
    assert!(profile.is_atomic("using_declaration"));
    assert!(profile.is_commutative("field_declaration_list"));
}

#[test]
fn test_svelte_profile() {
    let profile = Lang::Svelte.profile();
    assert!(profile.is_atomic("script_element"));
    assert!(profile.is_atomic("style_element"));
    assert!(profile.is_commutative("start_tag"));
}

#[test]
fn test_all_languages_have_valid_profiles() {
    for lang in Lang::all() {
        let profile = lang.profile();

        assert!(!profile.name.is_empty(), "Lang {lang:?} has empty name");

        assert!(
            !profile.extensions.is_empty() || !profile.file_names.is_empty(),
            "Lang {lang:?} has no extensions or file names"
        );
    }
}

#[test]
fn test_all_languages_can_create_parser() {
    for lang in Lang::all() {
        let ts_lang = lang.to_tree_sitter();
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&ts_lang)
            .expect("failed to set parser language");
    }
}

#[test]
fn test_parse_simple_code_samples() {
    let samples = [
        (Lang::Rust, "fn main() {}"),
        (Lang::JavaScript, "function main() {}"),
        (Lang::TypeScript, "function main(): void {}"),
        (Lang::Python, "def main():\n    pass"),
        (Lang::Go, "package main\nfunc main() {}"),
        (
            Lang::Java,
            "class Main { public static void main(String[] args) {} }",
        ),
        (Lang::Kotlin, "fun main() {}"),
        (Lang::Swift, "func main() {}"),
        (Lang::CSharp, "class Program { static void Main() {} }"),
        (Lang::Ruby, "def main\nend"),
        (Lang::Php, "<?php function main() {}"),
        (Lang::Bash, "#!/bin/bash\necho hello"),
        (Lang::Lua, "function main() end"),
        (Lang::C, "int main() { return 0; }"),
        (Lang::Cpp, "int main() { return 0; }"),
        (
            Lang::Scala,
            "object Main { def main(args: Array[String]): Unit = {} }",
        ),
        (Lang::Haskell, "main = return ()"),
        (Lang::Elixir, "defmodule Main do\nend"),
        (Lang::OCaml, "let main () = ()"),
        (Lang::Html, "<html><body></body></html>"),
        (Lang::Css, "body { color: red; }"),
        (
            Lang::Svelte,
            "<script>let count = 0;</script>\n<button>{count}</button>",
        ),
        (Lang::Json, r#"{"key": "value"}"#),
        (Lang::Toml, "[package]\nname = \"test\""),
        (Lang::Yaml, "key: value"),
        (Lang::Markdown, "# Hello\n\nWorld"),
        (Lang::Dockerfile, "FROM alpine:latest\nRUN echo hello"),
    ];

    for (lang, code) in samples {
        let ts_lang = lang.to_tree_sitter();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&ts_lang).unwrap();
        let tree = parser.parse(code, None);
        assert!(tree.is_some(), "Failed to parse {lang:?} code: {code}");
        let tree = tree.unwrap();

        let root = tree.root_node();
        assert!(
            root.child_count() > 0 || !root.byte_range().is_empty(),
            "Empty parse tree for {lang:?}"
        );
    }
}
