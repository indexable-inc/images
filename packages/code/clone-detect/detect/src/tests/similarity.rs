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

/// Two nodes with identical feature multisets should have similarity 1.0 under
/// either metric.
#[test]
fn identical_children() {
    let a = make_node_with_features(50, vec![100, 200, 300]);
    let b = make_node_with_features(50, vec![100, 200, 300]);
    assert!((jaccard(&a, &b) - 1.0).abs() < 0.001);
    assert!((overlap(&a, &b) - 1.0).abs() < 0.001);
}

/// Two nodes with completely different features should have similarity 0.0.
#[test]
fn completely_different_children() {
    let a = make_node_with_features(50, vec![100, 200, 300]);
    let b = make_node_with_features(50, vec![400, 500, 600]);
    assert!(jaccard(&a, &b) < 0.01);
    assert!(overlap(&a, &b) < 0.01);
}

/// Partial overlap: 2 of 3 features match. Jaccard = 2/4 = 0.5;
/// overlap = 2/min(3,3) = 0.667.
#[test]
fn partial_overlap() {
    let a = make_node_with_features(50, vec![100, 200, 300]);
    let b = make_node_with_features(55, vec![100, 200, 400]);
    assert!(
        (jaccard(&a, &b) - 0.5).abs() < 0.01,
        "jaccard = {}",
        jaccard(&a, &b)
    );
    assert!(
        (overlap(&a, &b) - 2.0 / 3.0).abs() < 0.01,
        "overlap = {}",
        overlap(&a, &b)
    );
}

/// One node is a superset (3 matching, 1 extra) — the containment case.
/// Jaccard = 3/4 = 0.75 (penalizes the extra feature); overlap = 3/3 = 1.0.
/// This asymmetry is what the overlap metric exists for.
#[test]
fn one_extra_child() {
    let a = make_node_with_features(50, vec![100, 200, 300]);
    let b = make_node_with_features(60, vec![100, 200, 300, 400]);
    assert!(
        (jaccard(&a, &b) - 0.75).abs() < 0.01,
        "jaccard = {}",
        jaccard(&a, &b)
    );
    assert!(
        (overlap(&a, &b) - 1.0).abs() < 0.01,
        "containment should give overlap 1.0, got {}",
        overlap(&a, &b)
    );
}

/// Both nodes have no features — fallback to node count ratio (min/max), which
/// is metric-independent.
#[test]
fn no_children_fallback() {
    let a = make_node_with_features(50, vec![]);
    let b = make_node_with_features(60, vec![]);
    assert!((jaccard(&a, &b) - 0.833).abs() < 0.01);
    assert!((overlap(&a, &b) - 0.833).abs() < 0.01);
}

/// Zero nodes in both — similarity 0.0.
#[test]
fn zero_nodes() {
    let a = make_node_with_features(0, vec![]);
    let b = make_node_with_features(0, vec![]);
    assert!(jaccard(&a, &b).abs() < 0.001);
    assert!(overlap(&a, &b).abs() < 0.001);
}

/// Duplicate features are treated as a multiset. `{100,100,200}` vs
/// `{100,200,200}`: intersection = min(2,1) + min(1,2) = 2. Jaccard = 2/4 = 0.5;
/// overlap = 2/min(3,3) = 0.667.
#[test]
fn multiset_duplicates() {
    let a = make_node_with_features(50, vec![100, 100, 200]);
    let b = make_node_with_features(50, vec![100, 200, 200]);
    assert!(
        (jaccard(&a, &b) - 0.5).abs() < 0.01,
        "jaccard = {}",
        jaccard(&a, &b)
    );
    assert!(
        (overlap(&a, &b) - 2.0 / 3.0).abs() < 0.01,
        "overlap = {}",
        overlap(&a, &b)
    );
}
