use crate::compute_similarity;

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

/// Two nodes with identical feature multisets should have similarity 1.0
#[test]
fn identical_children() {
    let a = make_node_with_features(50, vec![100, 200, 300]);
    let b = make_node_with_features(50, vec![100, 200, 300]);
    let sim = compute_similarity(&a, &b);
    assert!(
        (sim - 1.0).abs() < 0.001,
        "Identical features should give similarity 1.0, got {sim}"
    );
}

/// Two nodes with completely different features should have similarity 0.0
#[test]
fn completely_different_children() {
    let a = make_node_with_features(50, vec![100, 200, 300]);
    let b = make_node_with_features(50, vec![400, 500, 600]);
    let sim = compute_similarity(&a, &b);
    assert!(
        sim < 0.01,
        "Completely different features should give similarity ~0.0, got {sim}"
    );
}

/// Partial overlap: 2 out of 3 features match
#[test]
fn partial_overlap() {
    let a = make_node_with_features(50, vec![100, 200, 300]);
    let b = make_node_with_features(55, vec![100, 200, 400]);
    let sim = compute_similarity(&a, &b);
    // Jaccard of {100,200,300} and {100,200,400}: intersection=2, union=4 → 0.5
    assert!(
        (sim - 0.5).abs() < 0.01,
        "2/4 Jaccard overlap should give ~0.5, got {sim}"
    );
}

/// One node has an extra feature (superset): 3 matching, 1 extra
#[test]
fn one_extra_child() {
    let a = make_node_with_features(50, vec![100, 200, 300]);
    let b = make_node_with_features(60, vec![100, 200, 300, 400]);
    let sim = compute_similarity(&a, &b);
    // Jaccard: intersection=3, union=4 → 0.75
    assert!(
        (sim - 0.75).abs() < 0.01,
        "3/4 Jaccard overlap should give ~0.75, got {sim}"
    );
}

/// Both nodes have no features — fallback to node count ratio
#[test]
fn no_children_fallback() {
    let a = make_node_with_features(50, vec![]);
    let b = make_node_with_features(60, vec![]);
    let sim = compute_similarity(&a, &b);
    // Fallback: min/max = 50/60 ≈ 0.833
    assert!(
        (sim - 0.833).abs() < 0.01,
        "Empty features should fallback to node count ratio, got {sim}"
    );
}

/// Zero nodes in both — similarity 0.0
#[test]
fn zero_nodes() {
    let a = make_node_with_features(0, vec![]);
    let b = make_node_with_features(0, vec![]);
    let sim = compute_similarity(&a, &b);
    assert!(sim.abs() < 0.001, "Zero nodes should give 0.0, got {sim}");
}

/// Duplicate features should be treated as multiset (counted individually)
#[test]
fn multiset_duplicates() {
    let a = make_node_with_features(50, vec![100, 100, 200]);
    let b = make_node_with_features(50, vec![100, 200, 200]);
    let sim = compute_similarity(&a, &b);
    // Multiset intersection: min(2,1) for 100 = 1, min(1,2) for 200 = 1 → 2
    // Multiset union: max(2,1) for 100 = 2, max(1,2) for 200 = 2 → 4
    // Jaccard: 2/4 = 0.5
    assert!(
        (sim - 0.5).abs() < 0.01,
        "Multiset Jaccard with duplicate features should be 0.5, got {sim}"
    );
}
