//! The scoring walk: one descent per reported unit, computing every metric at
//! once.

use ast_merge_ast::Tree;
use ast_merge_langs::Lang;
use tree_sitter::Node;

use crate::kinds::{Profile, profile};

/// One reported unit of code and its measurements.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unit {
    /// The unit's first source line, trimmed. Used instead of a parsed name
    /// because it is language-independent, and because being the signature it
    /// changes exactly when the unit's contract changes, which is the property
    /// a baseline fingerprint wants.
    pub signature: String,
    /// 1-indexed, to match what an editor shows.
    pub start_line: usize,
    pub end_line: usize,
    pub lines: usize,
    /// Campbell cognitive complexity: the headline metric.
    pub cognitive: u32,
    /// McCabe cyclomatic complexity, computed as decision points plus one.
    /// Reported as a testability number, not a readability one: it has no
    /// measured correlation with comprehension time (Peitek et al., ICSE 2021).
    pub cyclomatic: u32,
    /// Deepest nesting of flow-breaking structures inside the unit.
    pub nesting: u32,
}

/// Cap on how much of the first line is kept as the signature.
const SIGNATURE_MAX: usize = 120;

/// Measure every unit in a parsed file.
///
/// Returns an empty vector for a language with no profile, so an unsupported
/// language contributes nothing rather than contributing zeros.
#[must_use]
pub fn measure(tree: &Tree, lang: Lang) -> Vec<Unit> {
    let Some(profile) = profile(lang) else {
        return Vec::new();
    };
    let mut units = Vec::new();
    collect(tree, profile, tree.root_node(), &mut units);
    units
}

/// Walk for unit roots, absorbing nested units into their enclosing unit.
fn collect(tree: &Tree, profile: &Profile, node: Node<'_>, units: &mut Vec<Unit>) {
    if is_unit(tree, profile, node) {
        units.push(score_unit(tree, profile, node));
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect(tree, profile, child, units);
    }
}

fn is_unit(tree: &Tree, profile: &Profile, node: Node<'_>) -> bool {
    let candidate =
        profile.units.contains(&node.kind()) || call_target_in(tree, node, profile.unit_calls);
    candidate && !is_namespace(profile, node)
}

/// A candidate whose value only groups other candidates is descended into
/// rather than reported, so a namespace does not absorb its members' scores.
fn is_namespace(profile: &Profile, node: Node<'_>) -> bool {
    if profile.namespace_values.is_empty() {
        return false;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| profile.namespace_values.contains(&child.kind()))
}

fn score_unit(tree: &Tree, profile: &Profile, node: Node<'_>) -> Unit {
    let mut acc = Acc::default();
    // The unit's own declaration never scores (Campbell exempts it), so the
    // walk starts at its children. That also makes the head rule uniform: a
    // child of the unit root is in the head, and stays there for as long as
    // the nodes are structural wrappers.
    let mut roots = node.walk();
    for child in node.named_children(&mut roots) {
        walk(tree, profile, child, 0, true, &mut acc);
    }

    let start_line = node.start_position().row + 1;
    let end_line = node.end_position().row + 1;
    Unit {
        signature: signature(tree, node),
        start_line,
        end_line,
        lines: end_line.saturating_sub(start_line) + 1,
        cognitive: acc.cognitive,
        cyclomatic: acc.decisions + 1,
        nesting: acc.max_nesting,
    }
}

fn signature(tree: &Tree, node: Node<'_>) -> String {
    let text = tree.node_text(node);
    let first = text.lines().next().unwrap_or_default().trim();
    match first.char_indices().nth(SIGNATURE_MAX) {
        Some((byte, _)) => format!("{}...", &first[..byte]),
        None => first.to_owned(),
    }
}

#[derive(Default)]
struct Acc {
    cognitive: u32,
    decisions: u32,
    max_nesting: u32,
}

/// `in_head` is true while the walk is still inside the unit's own signature:
/// the chain of lambdas a Nix binding uses for its parameters is the unit
/// itself, not a closure nested inside it, so it must not deepen. It goes
/// false at the first node that is not a structural wrapper.
fn walk(
    tree: &Tree,
    profile: &Profile,
    node: Node<'_>,
    nesting: u32,
    in_head: bool,
    acc: &mut Acc,
) {
    let kind = node.kind();
    let structural = profile.structural.contains(&kind);

    if profile.decisions.contains(&kind) || call_target_in(tree, node, profile.decision_calls) {
        acc.decisions += 1;
    }
    // McCabe counts each short-circuit operator: it makes its right-hand side
    // conditional, so it adds an independent path. Unlike the cognitive rule
    // below, a run of like operators is not folded.
    if logical_operator(tree, profile, node).is_some() {
        acc.decisions += 1;
    }

    let demoted = demoted_to_flat(tree, profile, node);
    let deepens = is_nesting(tree, profile, node) && !demoted;
    if deepens {
        acc.cognitive += 1 + nesting;
    } else if demoted || (profile.flat.contains(&kind) && !wraps_demoted(tree, profile, node)) {
        acc.cognitive += 1;
    }

    if counts_logical_run(tree, profile, node) {
        acc.cognitive += 1;
    }

    let child_nesting = if deepens || (structural && !in_head) {
        let deeper = nesting + 1;
        acc.max_nesting = acc.max_nesting.max(deeper);
        deeper
    } else {
        nesting
    };
    let child_in_head = in_head && structural;

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(tree, profile, child, child_nesting, child_in_head, acc);
    }
}

fn is_nesting(tree: &Tree, profile: &Profile, node: Node<'_>) -> bool {
    profile.nesting.contains(&node.kind()) || call_target_in(tree, node, profile.nesting_calls)
}

/// `else if` costs +1 but carries no nesting penalty: the reader already paid
/// for the branch when they read the `if` (Campbell, appendix B3). Grammars
/// spell it two ways, and both reduce to "this conditional is the alternative
/// arm of the one above it": Rust and TypeScript wrap it in an `else_clause`,
/// Go puts a bare `if_statement` in the parent's `alternative` field.
fn demoted_to_flat(tree: &Tree, profile: &Profile, node: Node<'_>) -> bool {
    if !is_nesting(tree, profile, node) {
        return false;
    }
    let Some(parent) = node.parent() else {
        return false;
    };
    if profile.flat.contains(&parent.kind()) {
        return true;
    }
    parent
        .child_by_field_name("alternative")
        .is_some_and(|alt| alt.id() == node.id())
}

/// An `else` wrapping an `else if` must not charge for both itself and the
/// conditional it wraps: Campbell scores the pair once. The wrapped
/// conditional carries the increment, so the wrapper stays silent.
fn wraps_demoted(tree: &Tree, profile: &Profile, node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| is_nesting(tree, profile, child))
}

/// A run of like logical operators is one increment, not one per operator, so
/// only the outermost node of each run is counted.
fn counts_logical_run(tree: &Tree, profile: &Profile, node: Node<'_>) -> bool {
    let Some(op) = logical_operator(tree, profile, node) else {
        return false;
    };
    let parent_op = node
        .parent()
        .and_then(|parent| logical_operator(tree, profile, parent));
    parent_op != Some(op)
}

fn logical_operator<'a>(
    tree: &'a Tree,
    profile: &'a Profile,
    node: Node<'_>,
) -> Option<&'a &'static str> {
    if !profile.logical.contains(&node.kind()) {
        return None;
    }
    // Most grammars name the operator field; tree-sitter-nix does not, so fall
    // back to scanning the unnamed children for a spelling we recognise.
    if let Some(field) = node.child_by_field_name("operator") {
        let text = tree.node_text(field);
        return profile.logical_ops.iter().find(|op| **op == text);
    }
    let mut cursor = node.walk();
    let children: Vec<_> = node.children(&mut cursor).collect();
    children.into_iter().find_map(|child| {
        if child.is_named() {
            return None;
        }
        let text = tree.node_text(child);
        profile.logical_ops.iter().find(|op| **op == text)
    })
}

/// True when `node` is a call whose target identifier is one of `targets`.
/// Elixir's grammar has no keyword node kinds, so `def`, `case` and `if` are
/// all ordinary calls distinguished only by the text of their target.
fn call_target_in(tree: &Tree, node: Node<'_>, targets: &[&str]) -> bool {
    if targets.is_empty() || node.kind() != "call" {
        return false;
    }
    node.child(0)
        .is_some_and(|target| targets.contains(&tree.node_text(target)))
}
