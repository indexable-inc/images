use super::*;

mod advanced;
mod merge;

#[test]
fn test_pcs_triple_creation() {
    let triple = PcsTriple::new(Some(0), None, 1);
    assert_eq!(triple.parent, Some(0));
    assert_eq!(triple.predecessor, None);
    assert_eq!(triple.successor, 1);
}

#[test]
fn test_changeset_add_and_get() {
    use ast_merge_ast::Revision;

    let mut changeset = ChangeSet::new();
    let triple = PcsTriple::new(None, None, 0);

    changeset.add(triple.clone(), Revision::Base);
    changeset.add(triple.clone(), Revision::Left);

    let revisions = changeset.get_revisions(&triple).unwrap();
    assert!(revisions.contains(&Revision::Base));
    assert!(revisions.contains(&Revision::Left));
    assert!(!revisions.contains(&Revision::Right));
}

#[test]
fn test_changeset_len() {
    use ast_merge_ast::Revision;

    let mut changeset = ChangeSet::new();
    assert!(changeset.is_empty());

    changeset.add(PcsTriple::new(None, None, 0), Revision::Base);
    assert_eq!(changeset.len(), 1);

    changeset.add(PcsTriple::new(None, None, 1), Revision::Left);
    assert_eq!(changeset.len(), 2);
}

#[test]
fn test_class_mapping_singleton() {
    use ast_merge_ast::Revision;

    use crate::mapping::RevisionNode;

    let mut mapping = Class::new();
    let class_id = mapping.add_singleton(RevisionNode {
        revision: Revision::Base,
        node_id: 0,
    });

    assert_eq!(
        mapping.get_class(RevisionNode {
            revision: Revision::Base,
            node_id: 0,
        }),
        Some(class_id)
    );
    assert!(mapping.get_class_members(class_id).is_some());
}

#[test]
fn test_class_mapping_combine() {
    use ast_merge_ast::Revision;

    use crate::mapping::{RevisionNode, RevisionNodePair};

    let mut mapping = Class::new();
    mapping.merge(&RevisionNodePair {
        a: RevisionNode {
            revision: Revision::Base,
            node_id: 0,
        },
        b: RevisionNode {
            revision: Revision::Left,
            node_id: 1,
        },
    });

    let class_base = mapping.get_class(RevisionNode {
        revision: Revision::Base,
        node_id: 0,
    });
    let class_left = mapping.get_class(RevisionNode {
        revision: Revision::Left,
        node_id: 1,
    });

    assert!(class_base.is_some());
    assert_eq!(class_base, class_left);
}

#[test]
fn test_class_mapping_is_in_all_revisions() {
    use ast_merge_ast::Revision;

    use crate::mapping::{RevisionNode, RevisionNodePair};

    let mut mapping = Class::new();
    mapping.merge(&RevisionNodePair {
        a: RevisionNode {
            revision: Revision::Base,
            node_id: 0,
        },
        b: RevisionNode {
            revision: Revision::Left,
            node_id: 0,
        },
    });
    mapping.merge(&RevisionNodePair {
        a: RevisionNode {
            revision: Revision::Base,
            node_id: 0,
        },
        b: RevisionNode {
            revision: Revision::Right,
            node_id: 0,
        },
    });

    let class_id = mapping
        .get_class(RevisionNode {
            revision: Revision::Base,
            node_id: 0,
        })
        .unwrap();
    assert!(mapping.is_in_all_revisions(class_id));
}

#[test]
fn test_conflict_region_creation() {
    let region = Region::new(10, 20, "hello".to_owned());
    assert_eq!(region.start, 10);
    assert_eq!(region.end, 20);
    assert_eq!(region.text, "hello");
}

#[test]
fn test_result_success() {
    let result = Result::success("merged content".to_owned());
    assert!(result.success);
    assert!(result.conflicts.is_empty());
    assert_eq!(result.content, "merged content");
}

#[test]
fn test_config_default() {
    let config = Config::default();
    assert_eq!(config.marker_size, 7);
    assert!(config.diff3_style);
}

#[test]
fn test_line_based_no_conflict() {
    let base = "line1\nline2\nline3\n";
    let left = "line1\nmodified\nline3\n";
    let right = "line1\nline2\nline3\n";

    let result = based(base, left, right);
    assert!(result.success);
    assert!(result.content.contains("modified"));
}

#[test]
fn test_line_based_with_conflict() {
    let base = "line1\nline2\nline3\n";
    let left = "line1\nleft_change\nline3\n";
    let right = "line1\nright_change\nline3\n";

    let result = based(base, left, right);
    assert!(!result.success);
    assert!(!result.conflicts.is_empty());
    assert!(result.content.contains("<<<<<<<"));
    assert!(result.content.contains(">>>>>>>"));
}

#[test]
fn test_line_based_same_change() {
    let base = "line1\nline2\n";
    let left = "line1\nchanged\n";
    let right = "line1\nchanged\n";

    let result = based(base, left, right);
    assert!(result.success);
    assert!(result.content.contains("changed"));
}

fn rust(base: &str, left: &str, right: &str) -> Result {
    let ts_lang = ast_merge_langs::Lang::Rust.to_tree_sitter();

    let base_parsed = ast_merge_ast::tree(base, &ts_lang).expect("parse base");
    let left_parsed = ast_merge_ast::tree(left, &ts_lang).expect("parse left");
    let right_parsed = ast_merge_ast::tree(right, &ts_lang).expect("parse right");

    let base_left_matching = ast_merge_matcher::compute(&base_parsed.tree, &left_parsed.tree);
    let base_right_matching = ast_merge_matcher::compute(&base_parsed.tree, &right_parsed.tree);

    let merger = ThreeWay::new(crate::engine::ThreeWayParams {
        trees: crate::engine::ThreeWayTrees {
            base: &base_parsed.tree,
            left: &left_parsed.tree,
            right: &right_parsed.tree,
        },
        base_left_matching,
        base_right_matching,
        config: Config::default(),
    });

    merger.merge()
}
