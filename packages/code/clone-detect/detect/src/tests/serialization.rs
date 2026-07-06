use std::path::PathBuf;

use crate::{
    ByteRange, CloneGroup, DetectionResult, DetectionStats, Fragment, Kind, LineRange, Type3Metric,
};

#[test]
fn type_to_string() {
    let type1 = Kind::Type1;
    let json = serde_json::to_string(&type1).unwrap();
    assert!(json.contains("type1"));

    let type3 = Kind::Type3 {
        similarity: 0.85,
        metric: Type3Metric::Overlap,
    };
    let json = serde_json::to_string(&type3).unwrap();
    assert!(json.contains("type3"));
    assert!(json.contains("0.85"));
    // The metric label must ride along so `similarity` is interpretable.
    assert!(json.contains("overlap"), "metric must be serialized: {json}");
}

#[test]
fn type_from_string() {
    let type1: Kind = serde_json::from_str("\"type1\"").unwrap();
    assert_eq!(type1, Kind::Type1);

    let type2: Kind = serde_json::from_str("\"type2\"").unwrap();
    assert_eq!(type2, Kind::Type2);

    let type3: Kind =
        serde_json::from_str("{\"type3\":{\"similarity\":0.75,\"metric\":\"jaccard\"}}").unwrap();
    assert!(matches!(
        type3,
        Kind::Type3 { similarity, metric }
            if (similarity - 0.75).abs() < 0.001 && metric == Type3Metric::Jaccard
    ));
}

#[test]
fn fragment_roundtrip() {
    let fragment = Fragment {
        file: PathBuf::from("/src/main.rs"),
        byte_range: ByteRange {
            start: 100,
            end: 200,
        },
        lines: LineRange { start: 10, end: 20 },
        kind: "function_item".to_owned(),
    };

    let json = serde_json::to_string(&fragment).unwrap();
    let deserialized: Fragment = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.file, fragment.file);
    assert_eq!(deserialized.byte_range, fragment.byte_range);
    assert_eq!(deserialized.lines, fragment.lines);
    assert_eq!(deserialized.kind, fragment.kind);
}

#[test]
fn detection_result_roundtrip() {
    let result = DetectionResult {
        instances: vec![CloneGroup {
            clone_type: Kind::Type1,
            fragments: vec![
                Fragment {
                    file: PathBuf::from("a.rs"),
                    byte_range: ByteRange { start: 0, end: 50 },
                    lines: LineRange { start: 0, end: 5 },
                    kind: "function_item".to_owned(),
                },
                Fragment {
                    file: PathBuf::from("b.rs"),
                    byte_range: ByteRange { start: 0, end: 50 },
                    lines: LineRange { start: 0, end: 5 },
                    kind: "function_item".to_owned(),
                },
            ],
        }],
        stats: DetectionStats {
            files_scanned: 2,
            nodes_analyzed: 10,
            total_lines: 100,
            duplicated_lines: 10,
            duplication_pct: 10.0,
            type1_groups: 1,
            type2_groups: 0,
            type3_groups: 0,
            sequence_groups: 0,
        },
    };

    let json = serde_json::to_string(&result).unwrap();
    let deserialized: DetectionResult = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.instances.len(), 1);
    assert_eq!(deserialized.stats.files_scanned, 2);
    assert_eq!(deserialized.stats.type1_groups, 1);
}
