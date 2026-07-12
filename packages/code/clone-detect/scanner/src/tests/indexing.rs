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

fn index_nodes(nodes: &[(u64, u64)]) -> Hash {
    let mut index = Hash::new();
    for (file_id, &(content_hash, normalized_hash)) in nodes.iter().enumerate() {
        index.add(
            &Entry {
                file_id,
                node_idx: 0,
            },
            &make_node(content_hash, normalized_hash, file_id * 100),
        );
    }
    index
}

#[test]
fn candidate_indexes_group_the_matching_hash() {
    let type1 = index_nodes(&[(123, 456), (123, 789), (999, 456)]);
    let type2 = index_nodes(&[(111, 456), (222, 456), (333, 789)]);

    assert_eq!(type1.type1_candidates().count(), 1);
    assert_eq!(*type1.type1_candidates().next().unwrap().hash, 123);
    assert_eq!(type2.type2_candidates().count(), 1);
    assert_eq!(*type2.type2_candidates().next().unwrap().hash, 456);
}

#[test]
fn no_candidates() {
    let index = index_nodes(&[(111, 111), (222, 222)]);
    assert!(index.type1_candidates().next().is_none());
    assert!(index.type2_candidates().next().is_none());
}
