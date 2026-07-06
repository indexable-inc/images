use std::path::PathBuf;

use clone_detect::{ByteRange, CloneGroup, Fragment, Kind, LineRange};

use super::{duplicated_lines, ratio_pct};

fn fragment(file: &str, start: usize, end: usize) -> Fragment {
    Fragment {
        file: PathBuf::from(file),
        byte_range: ByteRange { start: 0, end: 0 },
        lines: LineRange { start, end },
        kind: "function_item".to_owned(),
    }
}

fn group(fragments: Vec<Fragment>) -> CloneGroup {
    CloneGroup {
        clone_type: Kind::Type1,
        fragments,
    }
}

#[test]
fn duplicated_lines_skips_the_original_fragment() {
    // Two fragments, one line each: the first is the "original", so only the
    // second's line counts as duplicated.
    let g = group(vec![fragment("a.rs", 0, 4), fragment("b.rs", 10, 14)]);
    // b.rs rows 10..=14 => 5 lines.
    assert_eq!(duplicated_lines(&[g]), 5);
}

#[test]
fn duplicated_lines_dedups_overlapping_ranges_per_file() {
    // Two groups whose duplicate fragments overlap in the same file: rows
    // 10..=14 and 12..=16 in b.rs union to 10..=16 (7 distinct lines), not 10.
    let g1 = group(vec![fragment("a.rs", 0, 4), fragment("b.rs", 10, 14)]);
    let g2 = group(vec![fragment("a.rs", 20, 24), fragment("b.rs", 12, 16)]);
    assert_eq!(duplicated_lines(&[g1, g2]), 7);
}

#[test]
#[expect(
    clippy::float_cmp,
    reason = "0.0 is exactly representable and the guard returns it verbatim"
)]
fn ratio_pct_zero_denominator_is_zero() {
    assert_eq!(ratio_pct(5, 0), 0.0);
    assert_eq!(ratio_pct(0, 0), 0.0);
}

#[test]
#[expect(
    clippy::float_cmp,
    reason = "0.0 is exactly representable and 0/100 yields it verbatim"
)]
fn ratio_pct_basic() {
    assert!((ratio_pct(1, 4) - 25.0).abs() < 1e-9);
    assert_eq!(ratio_pct(0, 100), 0.0);
}
