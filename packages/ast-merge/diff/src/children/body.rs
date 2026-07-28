use crate::{
    engine::{ThreeWayNodes, ThreeWayTrees},
    lines::inner,
};

#[expect(
    clippy::string_slice,
    reason = "byte offsets from tree-sitter are guaranteed to be at UTF-8 char boundaries"
)]
pub fn try_reconcile_function(
    trees: &ThreeWayTrees<'_>,
    nodes: ThreeWayNodes<'_>,
) -> Option<String> {
    let ThreeWayTrees {
        base: base_tree,
        left: left_tree,
        right: right_tree,
    } = trees;
    let ThreeWayNodes {
        base: base_node,
        left: left_node,
        right: right_node,
    } = nodes;
    let base_body = base_node.child_by_field_name("body")?;
    let left_body = left_node.child_by_field_name("body")?;
    let right_body = right_node.child_by_field_name("body")?;

    let base_body_text = base_tree.node_text(base_body);
    let left_body_text = left_tree.node_text(left_body);
    let right_body_text = right_tree.node_text(right_body);

    let base_inner = strip_braces(base_body_text);
    let left_inner = strip_braces(left_body_text);
    let right_inner = strip_braces(right_body_text);

    let merge_result = inner(base_inner, left_inner, right_inner);

    if merge_result.has_conflict {
        return None;
    }

    let left_text = left_tree.node_text(left_node);
    let body_start = left_body.start_byte() - left_node.start_byte();
    let body_end = left_body.end_byte() - left_node.start_byte();

    let prefix = &left_text[..body_start];
    let suffix = &left_text[body_end..];

    let mut result = String::new();
    result.push_str(prefix);
    result.push_str("{\n");
    result.push_str(&merge_result.content);
    if !merge_result.content.ends_with('\n') {
        result.push('\n');
    }
    result.push('}');
    result.push_str(suffix);

    Some(result)
}

fn strip_braces(s: &str) -> &str {
    let s = s.trim();
    if s.strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
        .is_some()
    {
        s.get(1..s.len() - 1)
            .map_or(s, |inner| inner.trim_matches('\n'))
    } else {
        s
    }
}
