use tempfile::TempDir;

use super::helpers::{create_temp_file, scan_and_run};
use std::path::PathBuf;

use crate::{ByteRange, CloneGroup, DetectConfig, Fragment, Kind, LineRange, rank_by_impact};

#[test]
fn files_scanned() {
    let dir = TempDir::new().unwrap();

    create_temp_file(&dir, "file1.rs", "fn a() {}");
    create_temp_file(&dir, "file2.rs", "fn b() {}");
    create_temp_file(&dir, "file3.rs", "fn c() {}");

    let result = scan_and_run(&dir, &DetectConfig::default());

    assert_eq!(result.stats.files_scanned, 3);
}

#[test]
fn nodes_analyzed() {
    let dir = TempDir::new().unwrap();

    let code = r#"
fn func1() {
    println!("hello");
}

fn func2() {
    println!("world");
}
"#;
    create_temp_file(&dir, "file.rs", code);

    let result = scan_and_run(&dir, &DetectConfig::default());

    assert!(result.stats.nodes_analyzed >= 2);
}

fn fragment(file: &str, start: usize, end: usize) -> Fragment {
    Fragment {
        file: PathBuf::from(file),
        byte_range: ByteRange { start, end },
        lines: LineRange { start, end },
        kind: "function_item".to_owned(),
    }
}

#[test]
fn groups_are_ranked_by_removable_line_impact() {
    let low = CloneGroup {
        clone_type: Kind::Type1,
        fragments: vec![fragment("z.rs", 1, 3), fragment("y.rs", 1, 3)],
    };
    let high = CloneGroup {
        clone_type: Kind::Type2,
        fragments: vec![
            fragment("c.rs", 1, 10),
            fragment("b.rs", 1, 8),
            fragment("a.rs", 1, 6),
        ],
    };
    let mut groups = vec![low, high];

    rank_by_impact(&mut groups);

    assert_eq!(groups[0].line_impact(), 14);
    assert_eq!(groups[1].line_impact(), 3);
    assert_eq!(groups[0].fragments[0].file, PathBuf::from("c.rs"));
}
