use ast_merge_ast::{Tree, tree};
use ast_merge_langs::Lang;

use crate::{Info, Pragma, parse_text, ranges_overlap, scan};

fn parse(source: &str, language: Lang) -> Option<Tree> {
    let lang = language.to_tree_sitter();
    let result = tree(source, &lang);
    assert!(result.is_ok());
    match result {
        Ok(parsed) => Some(parsed.tree),
        Err(_) => None,
    }
}

fn scan_source(source: &str, language: Lang) -> Info {
    scan(&parse(source, language).expect("valid source"))
}

#[test]
fn test_parse_ignore() {
    assert_eq!(parse_text("// clone:ignore"), Some(Pragma::Ignore));
    assert_eq!(parse_text("# clone:ignore"), Some(Pragma::Ignore));
    assert_eq!(parse_text("/* clone:ignore */"), Some(Pragma::Ignore));
    assert_eq!(parse_text("-- clone:ignore"), Some(Pragma::Ignore));
}

#[test]
fn test_parse_ignore_file() {
    assert_eq!(parse_text("// clone:ignore-file"), Some(Pragma::IgnoreFile));
}

#[test]
fn test_parse_ignore_region() {
    assert_eq!(
        parse_text("// clone:ignore-start"),
        Some(Pragma::IgnoreStart)
    );
    assert_eq!(parse_text("// clone:ignore-end"), Some(Pragma::IgnoreEnd));
}

#[test]
fn test_parse_no_match() {
    assert_eq!(parse_text("// just a comment"), None);
    assert_eq!(parse_text("// TODO: fix this"), None);
    assert_eq!(parse_text("// clone: ignore"), None);
}

#[test]
fn ignore_file_is_language_independent() {
    let cases = [
        (Lang::Rust, "// clone:ignore-file\nfn foo() {}"),
        (Lang::Python, "# clone:ignore-file\ndef foo(): pass"),
        (
            Lang::JavaScript,
            "// clone:ignore-file\nfunction foo() {}",
        ),
    ];
    for (language, source) in cases {
        let tree = parse(source, language).expect("valid source");
        assert!(scan(&tree).ignore_file);
    }
}

#[test]
fn test_rust_ignore_next() {
    let source = r"
fn keep_this() {
    let x = 1;
}

// clone:ignore
fn ignore_this() {
    let y = 2;
}

fn also_keep() {
    let z = 3;
}
";
    let info = scan_source(source, Lang::Rust);

    assert!(!info.ignore_file);
    assert_eq!(info.ignored_ranges.len(), 1);

    let Some(ignored) = info.ignored_ranges.first() else {
        panic!("ignore pragma should produce one ignored range");
    };
    assert!(source[ignored.clone()].contains("ignore_this"));
    assert!(!source[ignored.clone()].contains("keep_this"));
    assert!(!source[ignored.clone()].contains("also_keep"));
}

#[test]
fn test_rust_ignore_region() {
    let source = r"
fn keep() {}

// clone:ignore-start
fn ignored1() {}
fn ignored2() {}
// clone:ignore-end

fn also_keep() {}
";
    let info = scan_source(source, Lang::Rust);

    assert!(!info.ignore_file);
    assert_eq!(info.ignored_ranges.len(), 1);

    let Some(ignored) = info.ignored_ranges.first() else {
        panic!("ignore region should produce one ignored range");
    };
    assert!(source[ignored.clone()].contains("ignored1"));
    assert!(source[ignored.clone()].contains("ignored2"));
    assert!(!source[ignored.clone()].contains("keep"));
    assert!(!source[ignored.clone()].contains("also_keep"));
}

#[test]
fn test_rust_no_pragmas() {
    let source = r"
fn foo() { let x = 1; }
fn bar() { let y = 2; }
";
    let info = scan_source(source, Lang::Rust);

    assert!(!info.ignore_file);
    assert!(info.ignored_ranges.is_empty());
}

#[test]
fn ignore_next_is_language_independent() {
    let cases = [
        (
            Lang::Python,
            "def keep(): pass\n# clone:ignore\ndef ignored(): pass",
        ),
        (
            Lang::JavaScript,
            "function keep() {}\n/* clone:ignore */\nfunction ignored() {}",
        ),
    ];
    for (language, source) in cases {
        let info = scan_source(source, language);
        assert!(!info.ignore_file);
        assert_eq!(info.ignored_ranges.len(), 1);
    }
}

#[test]
fn test_is_ignored_file() {
    let info = Info {
        ignore_file: true,
        ignored_ranges: vec![],
    };

    assert!(info.is_ignored(&(0..100)));
    assert!(info.is_ignored(&(50..150)));
}

#[test]
fn test_is_ignored_range() {
    let info = Info {
        ignore_file: false,
        ignored_ranges: vec![50..100],
    };

    assert!(!info.is_ignored(&(0..49)));
    assert!(info.is_ignored(&(40..60)));
    assert!(info.is_ignored(&(60..80)));
    assert!(info.is_ignored(&(90..110)));
    assert!(!info.is_ignored(&(100..150)));
}

#[test]
fn test_ranges_overlap() {
    assert!(ranges_overlap(&(0..10), &(5..15)));
    assert!(ranges_overlap(&(5..15), &(0..10)));
    assert!(ranges_overlap(&(0..10), &(0..10)));
    assert!(!ranges_overlap(&(0..10), &(10..20)));
    assert!(!ranges_overlap(&(0..10), &(20..30)));
}
