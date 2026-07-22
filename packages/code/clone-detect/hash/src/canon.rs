//! Per-language AST canonicalization applied ahead of the generic
//! normalizer (issue #3878, modeled on elixir-vibe's ExDNA).
//!
//! Language knowledge lives here, keyed off [`Lang`]; the recursion in
//! `normalize` only sees the canonical [`View`]. Semantically equivalent
//! surface forms map onto one view, so they hash equal:
//!
//! - Elixir pipes become plain calls: `x |> f(a)` views as `f(x, a)`. Every
//!   Elixir call is itself viewed as callee + flattened arguments so the two
//!   spellings meet in the middle (which also unifies `f(x, a)` with the
//!   paren-less `f x, a`).
//! - Elixir keyword pairs inside map/struct literals hash
//!   order-insensitively. Bare keyword lists and trailing keyword arguments
//!   stay verbatim: they are ordered data (pattern matches and
//!   `Keyword.pop_first/2` consume them positionally).
//! - Rust struct literals likewise: `field_initializer_list` sorts its
//!   initializers by field name.

use ast_merge_ast::Tree;
use ast_merge_langs::Lang;
use tree_sitter::Node;

/// Canonical shape for a node whose surface form should not affect its
/// normalized hash.
pub enum View<'t> {
    /// Hash as a call: one callee followed by arguments, punctuation and the
    /// pipe operator dropped.
    Call {
        target: Node<'t>,
        args: Vec<Node<'t>>,
    },
    /// Hash the node's kind with these children in canonical order,
    /// punctuation dropped.
    Ordered(Vec<Node<'t>>),
}

#[must_use]
pub fn view<'t>(lang: Lang, tree: &Tree, node: Node<'t>) -> Option<View<'t>> {
    match lang {
        Lang::Elixir => elixir(tree, node),
        Lang::Rust => rust(tree, node),
        _ => None,
    }
}

fn elixir<'t>(tree: &Tree, node: Node<'t>) -> Option<View<'t>> {
    match node.kind() {
        "binary_operator" => pipe(tree, node),
        "call" => {
            let (target, args) = call_parts(node)?;
            Some(View::Call { target, args })
        }
        "keywords" => keywords(tree, node),
        _ => None,
    }
}

/// Keyword pairs sort only inside map/struct literals (`map_content`
/// parent), where order is semantically irrelevant. A `list` or `arguments`
/// parent means an ordered keyword list, hashed verbatim so reordered
/// pipelines and option lists stay distinct (#3885 review).
fn keywords<'t>(tree: &Tree, node: Node<'t>) -> Option<View<'t>> {
    (node.parent()?.kind() == "map_content").then(|| sorted_by_key(tree, node, "key"))
}

/// `x |> f(a)` views as `f(x, a)`; a bare stage `x |> f` views as `f(x)`.
/// Chains canonicalize recursively because the left operand of the outer
/// pipe is the inner `binary_operator` node.
fn pipe<'t>(tree: &Tree, node: Node<'t>) -> Option<View<'t>> {
    let operator = node.child_by_field_name("operator")?;
    if tree.node_text(operator) != "|>" {
        return None;
    }
    let lhs = node.child_by_field_name("left")?;
    let rhs = node.child_by_field_name("right")?;

    if rhs.kind() == "call" {
        let (target, mut args) = call_parts(rhs)?;
        args.insert(0, lhs);
        return Some(View::Call { target, args });
    }
    Some(View::Call {
        target: rhs,
        args: vec![lhs],
    })
}

/// Flatten an Elixir `call` into callee + arguments: children of the
/// `arguments` node are spliced in directly, and any trailing `do_block`
/// rides along as a final argument.
fn call_parts(node: Node<'_>) -> Option<(Node<'_>, Vec<Node<'_>>)> {
    let target = node.child_by_field_name("target")?;
    let mut args = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.id() == target.id() {
            continue;
        }
        if child.kind() == "arguments" {
            let mut arguments = child.walk();
            args.extend(child.named_children(&mut arguments));
        } else {
            args.push(child);
        }
    }
    Some((target, args))
}

fn rust<'t>(tree: &Tree, node: Node<'t>) -> Option<View<'t>> {
    (node.kind() == "field_initializer_list").then(|| sorted_by_key(tree, node, "field"))
}

/// Named children sorted by the text of their `key_field` child, falling
/// back to the child's own text (Rust shorthand and base initializers have
/// no `field` child). The sort is stable, so duplicate keys keep their
/// source order.
///
/// The sort key is raw source text, evaluated before identifier
/// renumbering, which trades away one Type-II case on purpose: a clone that
/// consistently renames the keys themselves (two different Rust struct
/// types) can reorder under sorting and hash apart. That is a missed clone,
/// never a false match; sorting on normalized keys would need a renumbering
/// pre-pass whose numbering itself depends on traversal order (#3885
/// review).
fn sorted_by_key<'t>(tree: &Tree, node: Node<'t>, key_field: &str) -> View<'t> {
    let mut cursor = node.walk();
    let mut children: Vec<Node<'t>> = node.named_children(&mut cursor).collect();
    children.sort_by_key(|child| {
        let key = child.child_by_field_name(key_field).unwrap_or(*child);
        tree.node_text(key)
    });
    View::Ordered(children)
}
