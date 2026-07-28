//! The detector must never pair two overlapping byte ranges of a single file
//! against each other. This invariant holds for every detection mode; each
//! case below crafts a one-file fixture that gives its detector within-file
//! candidates, driven through
//! [`clone_test_support::assert_single_file_has_no_overlapping_fragments`].

use clone_detect::{DetectConfig, Kind};
use clone_test_support::assert_single_file_has_no_overlapping_fragments;

/// A nested function whose subtrees give the Type-3 detector overlapping
/// same-file candidates.
const TYPE3_FIXTURE: &str = r"
fn process(values: &[i32]) -> i32 {
    let mut total = 0;
    for value in values {
        if *value > 0 {
            total += value;
        }
    }
    total
}
";

/// A function with a run of statements that gives the sequence detector
/// overlapping same-file windows.
const SEQUENCE_FIXTURE: &str = r#"
fn process(values: &[i32]) -> i32 {
    let mut total = 0;
    for value in values {
        total += value;
    }
    println!("{total}");
    total
}
"#;

/// One detection mode: fixture code, the config that enables the detector,
/// and the selector for the pair kind it reports.
type Case = (&'static str, DetectConfig, fn(&Kind) -> bool);

#[test]
fn detectors_never_compare_overlapping_regions_of_one_file() {
    let cases: [Case; 2] = [
        (
            TYPE3_FIXTURE,
            DetectConfig {
                enable_type3: true,
                type3_threshold: 0.5,
                ..DetectConfig::default()
            },
            |kind| matches!(kind, Kind::Type3 { .. }),
        ),
        (
            SEQUENCE_FIXTURE,
            DetectConfig {
                enable_sequences: true,
                sequence_window_size: 2,
                ..DetectConfig::default()
            },
            |kind| matches!(kind, Kind::Sequence { .. }),
        ),
    ];

    for (code, config, selected) in cases {
        assert_single_file_has_no_overlapping_fragments(code, &config, selected);
    }
}
