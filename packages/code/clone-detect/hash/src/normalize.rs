use std::hash::{Hash, Hasher};

use ast_merge_ast::Tree;
use ast_merge_langs::Lang;
use rustc_hash::{FxHashMap, FxHasher};

use crate::{
    canon::{self, View},
    kinds::{is_identifier, is_normalizable},
};

const LITERAL_PLACEHOLDER: u64 = 0xDEAD_BEEF;

/// Virtual node kind hashed in place of a grammar kind for canonical calls.
/// Tree-sitter kind names never contain `:`, so a canonical call cannot
/// collide with a verbatim node.
const CANON_CALL_KIND: &str = "canon:call";

#[must_use]
pub fn hash(tree: &Tree, lang: Lang, node: tree_sitter::Node<'_>) -> u64 {
    let mut hasher = FxHasher::default();
    let mut state = State::default();
    recursive(
        &mut Context {
            tree,
            lang,
            hasher: &mut hasher,
            state: &mut state,
        },
        node,
    );
    hasher.finish()
}

#[derive(Default)]
struct State<'a> {
    identifier_map: FxHashMap<&'a str, u32>,
    next_id: u32,
}

impl<'a> State<'a> {
    fn get_or_assign(&mut self, text: &'a str) -> u32 {
        *self.identifier_map.entry(text).or_insert_with(|| {
            let id = self.next_id;
            self.next_id += 1;
            id
        })
    }
}

struct Context<'a, 'b> {
    tree: &'a Tree,
    lang: Lang,
    hasher: &'b mut FxHasher,
    state: &'b mut State<'a>,
}

fn recursive(ctx: &mut Context<'_, '_>, node: tree_sitter::Node<'_>) {
    match canon::view(ctx.lang, ctx.tree, node) {
        Some(View::Call { target, args }) => {
            CANON_CALL_KIND.hash(ctx.hasher);
            recursive(ctx, target);
            args.len().hash(ctx.hasher);
            for arg in args {
                recursive(ctx, arg);
            }
        }
        Some(View::Ordered(children)) => {
            node.kind().hash(ctx.hasher);
            children.len().hash(ctx.hasher);
            for child in children {
                recursive(ctx, child);
            }
        }
        None => verbatim(ctx, node),
    }
}

fn verbatim(ctx: &mut Context<'_, '_>, node: tree_sitter::Node<'_>) {
    let kind = node.kind();
    kind.hash(ctx.hasher);

    if node.child_count() == 0 {
        if is_normalizable(kind) {
            let text = ctx.tree.node_text(node);
            if is_identifier(kind) {
                let id = ctx.state.get_or_assign(text);
                id.hash(ctx.hasher);
            } else {
                LITERAL_PLACEHOLDER.hash(ctx.hasher);
            }
        } else {
            ctx.tree.node_text(node).hash(ctx.hasher);
        }
    } else {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            recursive(ctx, child);
        }
    }
}
