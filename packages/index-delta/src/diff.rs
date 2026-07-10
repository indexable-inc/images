//! The logical differ: one algorithm over `serde_json::Value` serves every
//! structured format; only the *addressing* differs (JSON Pointer for
//! json/yaml/plist, dotted paths for toml/keyvalue). Text files fall back to
//! a single unified-diff op — still queued, never auto-merged.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::store;
use crate::value::{self, Doc, Format};

/// One logical edit. `Add`/`Remove`/`Replace` are addressed ops a model can
/// reason about (and `apply-ops` can replay onto a repo source file);
/// `Text`/`Binary` are the unaddressed fallbacks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Op {
    Add {
        path: String,
        value: Value,
    },
    Remove {
        path: String,
        from: Value,
    },
    Replace {
        path: String,
        from: Value,
        to: Value,
    },
    /// Unified diff of the whole file — the fallback for `text` format and
    /// for structured files whose upper failed to parse.
    Text {
        diff: String,
    },
    /// Contents differ and at least one side is not UTF-8.
    Binary,
}

impl Op {
    /// The address an op touches, when it has one.
    pub fn address(&self) -> Option<&str> {
        match self {
            Self::Add { path, .. } | Self::Remove { path, .. } | Self::Replace { path, .. } => {
                Some(path)
            }
            Self::Text { .. } | Self::Binary => None,
        }
    }
}

#[derive(Clone, Copy)]
enum Addressing {
    /// RFC 6901 JSON Pointer (`/profiles/0/name`).
    Pointer,
    /// Dotted key paths (`section.key`), matching how toml/keyvalue files
    /// are actually written and edited.
    Dotted,
}

const fn addressing(format: Format) -> Addressing {
    match format {
        Format::Toml | Format::Keyvalue => Addressing::Dotted,
        Format::Json | Format::Yaml | Format::Plist | Format::Text => Addressing::Pointer,
    }
}

const fn separator(format: Format) -> char {
    match addressing(format) {
        Addressing::Pointer => '/',
        Addressing::Dotted => '.',
    }
}

/// Diff two byte snapshots of a file under its recorded format. An empty
/// result means *logically equal* — formatting-only changes (reordered JSON
/// whitespace, requoted YAML) produce no ops and count as clean.
pub fn diff_bytes(format: Format, old: &[u8], new: &[u8]) -> Vec<Op> {
    if old == new {
        return Vec::new();
    }
    match (value::load(format, old), value::load(format, new)) {
        (Doc::Structured(old_value), Doc::Structured(new_value)) => {
            let mut ops = Vec::new();
            let mut path = Vec::new();
            diff_values(
                addressing(format),
                &mut path,
                &old_value,
                &new_value,
                &mut ops,
            );
            ops
        }
        (Doc::Text(old_text), Doc::Text(new_text)) => text_ops(&old_text, &new_text),
        (Doc::Binary(old_bytes), Doc::Binary(new_bytes)) if old_bytes == new_bytes => Vec::new(),
        _ => fallback_ops(old, new),
    }
}

/// Two snapshots are logically equal when their diff is empty.
pub fn logically_equal(format: Format, old: &[u8], new: &[u8]) -> bool {
    diff_bytes(format, old, new).is_empty()
}

/// Stable fingerprint of a set of ops, used by `snooze`: the file stays
/// silenced until its diff changes.
pub fn fingerprint(ops: &[Op]) -> String {
    let text = serde_json::to_string(ops).unwrap_or_default();
    store::hex(&Sha256::digest(text.as_bytes()))
}

/// Addresses touched by both sides of a conflict — equal paths or one being
/// a prefix of the other. Empty overlap means the two diffs are disjoint and
/// trivially resolvable; non-empty is where judgment is needed. Two text
/// diffs always overlap (there is no addressing to prove them disjoint).
pub fn overlap(format: Format, yours: &[Op], incoming: &[Op]) -> Vec<String> {
    let unaddressed = |ops: &[Op]| ops.iter().any(|op| op.address().is_none());
    if !yours.is_empty() && !incoming.is_empty() && (unaddressed(yours) || unaddressed(incoming)) {
        return vec!["(entire file)".to_owned()];
    }
    let sep = separator(format);
    let mut hits: Vec<String> = yours
        .iter()
        .filter_map(Op::address)
        .filter(|yours_path| {
            incoming
                .iter()
                .filter_map(Op::address)
                .any(|incoming_path| related(sep, yours_path, incoming_path))
        })
        .map(str::to_owned)
        .collect();
    hits.dedup();
    hits
}

fn related(sep: char, a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let prefixed = |longer: &str, shorter: &str| {
        longer
            .strip_prefix(shorter)
            .is_some_and(|rest| rest.starts_with(sep))
    };
    prefixed(a, b) || prefixed(b, a)
}

fn fallback_ops(old: &[u8], new: &[u8]) -> Vec<Op> {
    match (std::str::from_utf8(old), std::str::from_utf8(new)) {
        (Ok(old_text), Ok(new_text)) => text_ops(old_text, new_text),
        _ => vec![Op::Binary],
    }
}

fn text_ops(old: &str, new: &str) -> Vec<Op> {
    if old == new {
        return Vec::new();
    }
    let diff = similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(2)
        .to_string();
    vec![Op::Text { diff }]
}

fn diff_values(
    addr: Addressing,
    path: &mut Vec<String>,
    old: &Value,
    new: &Value,
    ops: &mut Vec<Op>,
) {
    if old == new {
        return;
    }
    match (old, new) {
        (Value::Object(old_map), Value::Object(new_map)) => {
            for (key, old_item) in old_map {
                path.push(key.clone());
                match new_map.get(key) {
                    Some(new_item) => diff_values(addr, path, old_item, new_item, ops),
                    None => ops.push(Op::Remove {
                        path: render(addr, path),
                        from: old_item.clone(),
                    }),
                }
                path.pop();
            }
            let added = new_map
                .iter()
                .filter(|(key, _)| !old_map.contains_key(*key))
                .map(|(key, value)| Leaf {
                    segment: key.clone(),
                    value,
                });
            push_leaf_ops(addr, path, ops, added, |path, value| Op::Add {
                path,
                value,
            });
        }
        (Value::Array(old_items), Value::Array(new_items)) => {
            diff_arrays(addr, path, old_items, new_items, ops);
        }
        _ => ops.push(Op::Replace {
            path: render(addr, path),
            from: old.clone(),
            to: new.clone(),
        }),
    }
}

/// Arrays are diffed positionally: same length recurses per index, a pure
/// append/truncate becomes tail adds/removes (the common "added a rule"
/// case), anything else replaces the whole array. Deliberately no LCS —
/// deterministic, and a model resolving the queue prefers one honest
/// replace over clever-but-wrong moves.
fn diff_arrays(
    addr: Addressing,
    path: &mut Vec<String>,
    old_items: &[Value],
    new_items: &[Value],
    ops: &mut Vec<Op>,
) {
    let shared = old_items.len().min(new_items.len());
    let prefix_equal = old_items[..shared] == new_items[..shared];
    if old_items.len() == new_items.len() {
        for (index, (old_item, new_item)) in old_items.iter().zip(new_items).enumerate() {
            path.push(index.to_string());
            diff_values(addr, path, old_item, new_item, ops);
            path.pop();
        }
    } else if prefix_equal && new_items.len() > old_items.len() {
        let appended = indexed(new_items, old_items.len());
        push_leaf_ops(addr, path, ops, appended, |path, value| Op::Add {
            path,
            value,
        });
    } else if prefix_equal {
        // Descending so the ops stay valid when applied in order.
        let truncated = indexed(old_items, new_items.len()).rev();
        push_leaf_ops(addr, path, ops, truncated, |path, from| Op::Remove {
            path,
            from,
        });
    } else {
        ops.push(Op::Replace {
            path: render(addr, path),
            from: Value::Array(old_items.to_vec()),
            to: Value::Array(new_items.to_vec()),
        });
    }
}

struct Leaf<'v> {
    segment: String,
    value: &'v Value,
}

fn indexed(items: &[Value], from: usize) -> impl DoubleEndedIterator<Item = Leaf<'_>> {
    items
        .iter()
        .enumerate()
        .skip(from)
        .map(|(index, value)| Leaf {
            segment: index.to_string(),
            value,
        })
}

/// Emit one leaf op per `(segment, value)` pair, each addressed one level
/// below the current `path`. Shared by object-key adds and array tail
/// adds/removes so the push/render/pop dance lives in exactly one place.
fn push_leaf_ops<'v>(
    addr: Addressing,
    path: &mut Vec<String>,
    ops: &mut Vec<Op>,
    items: impl Iterator<Item = Leaf<'v>>,
    make: fn(String, Value) -> Op,
) {
    for leaf in items {
        path.push(leaf.segment);
        ops.push(make(render(addr, path), leaf.value.clone()));
        path.pop();
    }
}

fn render(addr: Addressing, segments: &[String]) -> String {
    match addr {
        Addressing::Pointer => {
            let mut out = String::new();
            for segment in segments {
                out.push('/');
                out.push_str(&segment.replace('~', "~0").replace('/', "~1"));
            }
            out
        }
        Addressing::Dotted => segments.join("."),
    }
}

/// Parse a rendered address back into segments, for `apply-ops`.
pub fn parse_address(format: Format, path: &str) -> Vec<String> {
    match addressing(format) {
        Addressing::Pointer => path
            .split('/')
            .skip(1)
            .map(|segment| segment.replace("~1", "/").replace("~0", "~"))
            .collect(),
        Addressing::Dotted => path.split('.').map(str::to_owned).collect(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn ops(format: Format, old: &str, new: &str) -> Vec<Op> {
        diff_bytes(format, old.as_bytes(), new.as_bytes())
    }

    #[test]
    fn formatting_only_changes_are_clean() {
        assert!(
            ops(
                Format::Json,
                "{\"a\":1,\"b\":2}",
                "{\n  \"a\": 1,\n  \"b\": 2\n}"
            )
            .is_empty()
        );
    }

    #[test]
    fn object_add_remove_replace() {
        let found = ops(
            Format::Json,
            r#"{"keep": 1, "drop": 2, "change": {"deep": true}}"#,
            r#"{"keep": 1, "change": {"deep": false}, "new": [1]}"#,
        );
        assert_eq!(
            found,
            vec![
                Op::Remove {
                    path: "/drop".to_owned(),
                    from: json!(2)
                },
                Op::Replace {
                    path: "/change/deep".to_owned(),
                    from: json!(true),
                    to: json!(false)
                },
                Op::Add {
                    path: "/new".to_owned(),
                    value: json!([1])
                },
            ]
        );
    }

    #[test]
    fn array_append_and_rewrite() {
        let appended = ops(
            Format::Json,
            r#"{"rules": [1, 2]}"#,
            r#"{"rules": [1, 2, 3]}"#,
        );
        assert_eq!(
            appended,
            vec![Op::Add {
                path: "/rules/2".to_owned(),
                value: json!(3)
            }]
        );
        let rewritten = ops(Format::Json, r#"{"rules": [1, 2]}"#, r#"{"rules": [2]}"#);
        assert_eq!(
            rewritten,
            vec![Op::Replace {
                path: "/rules".to_owned(),
                from: json!([1, 2]),
                to: json!([2])
            }]
        );
    }

    #[test]
    fn keyvalue_uses_dotted_addresses() {
        let found = ops(
            Format::Keyvalue,
            "font-size = 13\n[sec]\nk = v\n",
            "font-size = 14\n[sec]\nk = v\n",
        );
        assert_eq!(
            found,
            vec![Op::Replace {
                path: "font-size".to_owned(),
                from: json!("13"),
                to: json!("14")
            }]
        );
    }

    #[test]
    fn text_falls_back_to_unified_diff() {
        let found = ops(Format::Text, "a\nb\n", "a\nc\n");
        let [Op::Text { diff }] = found.as_slice() else {
            panic!("expected one text op, got {found:?}");
        };
        assert!(diff.contains("-b"), "diff was: {diff}");
        assert!(diff.contains("+c"), "diff was: {diff}");
    }

    #[test]
    fn overlap_matches_prefixes_only() {
        let yours = vec![Op::Replace {
            path: "/profiles/0/name".to_owned(),
            from: json!("a"),
            to: json!("b"),
        }];
        let disjoint = vec![Op::Replace {
            path: "/profiles/1".to_owned(),
            from: json!(1),
            to: json!(2),
        }];
        let ancestor = vec![Op::Remove {
            path: "/profiles/0".to_owned(),
            from: json!({}),
        }];
        assert!(overlap(Format::Json, &yours, &disjoint).is_empty());
        assert_eq!(
            overlap(Format::Json, &yours, &ancestor),
            vec!["/profiles/0/name".to_owned()]
        );
    }

    #[test]
    fn pointer_round_trips_through_parse() {
        let segments = parse_address(Format::Json, "/a~1b/c~0d/0");
        assert_eq!(
            segments,
            vec!["a/b".to_owned(), "c~d".to_owned(), "0".to_owned()]
        );
    }
}
