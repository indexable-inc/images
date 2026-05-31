use clone_hash::NodeInfo;

use crate::{Hash, index::Entry};

#[test]
fn new_index_is_empty() {
    let index = Hash::new();
    assert!(index.content_index.is_empty());
    assert!(index.normalized_index.is_empty());
}

#[test]
fn add_populates_both_indexes() {
    let mut index = Hash::new();
    let node = NodeInfo {
        content_hash: 123,
        normalized_hash: 456,
        kind: "function_item",
        byte_range: 0..10,
        start_line: 0,
        end_line: 5,
        node_count: 10,
        children: vec![],
        subtree_features: vec![],
    };

    index.add(
        &Entry {
            file_id: 0,
            node_idx: 0,
        },
        &node,
    );

    assert!(index.content_index.contains_key(&123));
    assert!(index.normalized_index.contains_key(&456));
}

fn make_node(content_hash: u64, normalized_hash: u64, byte_start: usize) -> NodeInfo {
    NodeInfo {
        content_hash,
        normalized_hash,
        kind: "function_item",
        byte_range: byte_start..(byte_start + 10),
        start_line: byte_start / 10,
        end_line: byte_start / 10 + 5,
        node_count: 10,
        children: vec![],
        subtree_features: vec![],
    }
}

#[test]
fn type1_candidates() {
    let mut index = Hash::new();

    let node1 = make_node(123, 456, 0);
    let node2 = make_node(123, 789, 100);
    let node3 = make_node(999, 456, 200);

    index.add(
        &Entry {
            file_id: 0,
            node_idx: 0,
        },
        &node1,
    );
    index.add(
        &Entry {
            file_id: 1,
            node_idx: 0,
        },
        &node2,
    );
    index.add(
        &Entry {
            file_id: 2,
            node_idx: 0,
        },
        &node3,
    );

    let candidates: Vec<_> = index.type1_candidates().collect();
    assert_eq!(candidates.len(), 1);
    assert_eq!(*candidates.first().unwrap().hash, 123);
}

#[test]
fn type2_candidates() {
    let mut index = Hash::new();

    let node1 = make_node(111, 456, 0);
    let node2 = make_node(222, 456, 100);
    let node3 = make_node(333, 789, 200);

    index.add(
        &Entry {
            file_id: 0,
            node_idx: 0,
        },
        &node1,
    );
    index.add(
        &Entry {
            file_id: 1,
            node_idx: 0,
        },
        &node2,
    );
    index.add(
        &Entry {
            file_id: 2,
            node_idx: 0,
        },
        &node3,
    );

    let candidates: Vec<_> = index.type2_candidates().collect();
    assert_eq!(candidates.len(), 1);
    assert_eq!(*candidates.first().unwrap().hash, 456);
}

#[test]
fn no_candidates() {
    let mut index = Hash::new();

    let node1 = make_node(111, 111, 0);
    let node2 = make_node(222, 222, 100);

    index.add(
        &Entry {
            file_id: 0,
            node_idx: 0,
        },
        &node1,
    );
    index.add(
        &Entry {
            file_id: 1,
            node_idx: 0,
        },
        &node2,
    );
    assert!(index.type1_candidates().next().is_none());
    assert!(index.type2_candidates().next().is_none());
}
