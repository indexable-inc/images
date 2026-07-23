use crate::{Type3Metric, compute_similarity_with};

fn make_node_with_features(node_count: usize, features: Vec<u64>) -> clone_hash::NodeInfo {
    clone_hash::NodeInfo {
        content_hash: 1,
        normalized_hash: 2,
        kind: "function_item",
        byte_range: 0..100,
        start_line: 0,
        end_line: 10,
        node_count,
        children: vec![],
        subtree_features: features,
    }
}

fn jaccard(a: &clone_hash::NodeInfo, b: &clone_hash::NodeInfo) -> f64 {
    compute_similarity_with(a, b, Type3Metric::Jaccard)
}

fn overlap(a: &clone_hash::NodeInfo, b: &clone_hash::NodeInfo) -> f64 {
    compute_similarity_with(a, b, Type3Metric::Overlap)
}

#[test]
fn feature_similarity_metrics_cover_sets_multisets_and_empty_fallbacks() {
    let cases = [
        (
            "identical",
            50,
            vec![100, 200, 300],
            50,
            vec![100, 200, 300],
            1.0,
            1.0,
        ),
        (
            "disjoint",
            50,
            vec![100, 200, 300],
            50,
            vec![400, 500, 600],
            0.0,
            0.0,
        ),
        (
            "partial",
            50,
            vec![100, 200, 300],
            55,
            vec![100, 200, 400],
            0.5,
            2.0 / 3.0,
        ),
        (
            "contained",
            50,
            vec![100, 200, 300],
            60,
            vec![100, 200, 300, 400],
            0.75,
            1.0,
        ),
        ("empty fallback", 50, vec![], 60, vec![], 0.833, 0.833),
        ("zero nodes", 0, vec![], 0, vec![], 0.0, 0.0),
        (
            "multiset",
            50,
            vec![100, 100, 200],
            50,
            vec![100, 200, 200],
            0.5,
            2.0 / 3.0,
        ),
    ];
    for (name, a_count, a_features, b_count, b_features, expected_jaccard, expected_overlap) in
        cases
    {
        let a = make_node_with_features(a_count, a_features);
        let b = make_node_with_features(b_count, b_features);
        assert!(
            (jaccard(&a, &b) - expected_jaccard).abs() < 0.01,
            "{name}: jaccard"
        );
        assert!(
            (overlap(&a, &b) - expected_overlap).abs() < 0.01,
            "{name}: overlap"
        );
    }
}
