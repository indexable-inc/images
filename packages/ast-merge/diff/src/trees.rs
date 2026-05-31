pub fn collect_nodes<'a>(node: tree_sitter::Node<'a>, nodes: &mut Vec<tree_sitter::Node<'a>>) {
    nodes.push(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_nodes(child, nodes);
    }
}

pub fn find_predecessor(
    parent_node: tree_sitter::Node<'_>,
    node: tree_sitter::Node<'_>,
    node_ids: &[tree_sitter::Node<'_>],
) -> Option<usize> {
    let mut cursor = parent_node.walk();
    let siblings: Vec<_> = parent_node.children(&mut cursor).collect();
    let pos = siblings.iter().position(|s| s.id() == node.id())?;

    if pos == 0 {
        return None;
    }

    let prev = siblings.get(pos - 1)?;
    node_ids.iter().position(|n| n.id() == prev.id())
}
