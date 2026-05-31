use crate::matching::Map;

#[derive(Clone, Copy)]
pub struct SiblingNodes<'a, 'b> {
    pub ids: &'b [usize],
    pub nodes: &'b [tree_sitter::Node<'a>],
}

pub struct SubtreesInput<'a, 'b> {
    pub node_a: Option<tree_sitter::Node<'a>>,
    pub node_b: Option<tree_sitter::Node<'a>>,
    pub nodes_a: &'b [tree_sitter::Node<'a>],
    pub nodes_b: &'b [tree_sitter::Node<'a>],
}

pub fn node_height(node: tree_sitter::Node<'_>) -> usize {
    if node.child_count() == 0 {
        return 1;
    }

    let mut max_child_height = 0;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        max_child_height = max_child_height.max(node_height(child));
    }

    max_child_height + 1
}

pub fn assign_node_ids<'a>(node: tree_sitter::Node<'a>, ids: &mut Vec<tree_sitter::Node<'a>>) {
    ids.push(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        assign_node_ids(child, ids);
    }
}

pub fn subtrees(input: &SubtreesInput<'_, '_>, matching: &mut Map) {
    let (Some(node_a), Some(node_b)) = (input.node_a, input.node_b) else {
        return;
    };

    let a_id = input.nodes_a.iter().position(|n| n.id() == node_a.id());
    let b_id = input.nodes_b.iter().position(|n| n.id() == node_b.id());

    if let (Some(a_id), Some(b_id)) = (a_id, b_id) {
        if !matching.is_matched_a(a_id) && !matching.is_matched_b(b_id) {
            matching.add_match(a_id, b_id);
        }
    }

    let mut cursor_a = node_a.walk();
    let mut cursor_b = node_b.walk();
    let children_b: Vec<_> = node_b.children(&mut cursor_b).collect();

    for (child_a, child_b) in node_a.children(&mut cursor_a).zip(children_b) {
        subtrees(
            &SubtreesInput {
                node_a: Some(child_a),
                node_b: Some(child_b),
                nodes_a: input.nodes_a,
                nodes_b: input.nodes_b,
            },
            matching,
        );
    }
}
