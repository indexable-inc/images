//! Extract a namespace name and its `:require`/`:use` edges from an `ns` form.
//!
//! The shape is `(ns name docstring? attr-map? references*)`, so the docstring
//! and metadata map are simply skipped over: only the reference clauses matter.
//! Libspec grammar follows `clojure.core/load-libs`, including prefix lists.

use crate::reader::{Form, Kind, ReadError, Reader};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NsForm {
    pub name: String,
    /// Every namespace named by a `:require` or `:use` clause, in source order
    /// and not yet filtered against the graph.
    pub requires: Vec<String>,
}

/// Clauses that name namespaces to load. `:import` names Java classes and
/// `:refer-clojure`/`:gen-class`/`:load` name nothing we build, so they are not
/// here.
const LOADING_CLAUSES: [&str; 2] = ["require", "use"];

pub fn parse(src: &str) -> Result<NsForm, ReadError> {
    let mut reader = Reader::new(src);
    let items = loop {
        let Some(form) = reader.next_form()? else {
            return Err(ReadError::new(
                src.len(),
                "no `ns` form in this file; every unit source must declare one",
            ));
        };
        if let Kind::List(items) = form.kind
            && items.first().and_then(Form::symbol) == Some("ns")
        {
            break items;
        }
    };

    let mut rest = items.into_iter().skip(1);
    let name_form = rest
        .next()
        .ok_or_else(|| ReadError::new(0, "`ns` form has no namespace name"))?;
    let name = namespace_symbol(&name_form, "a namespace name must be a symbol")?;

    let mut requires = Vec::new();
    for clause in rest {
        let Kind::List(parts) = &clause.kind else {
            continue;
        };
        let Some(head) = parts.first() else {
            continue;
        };
        // `(:require ...)` is idiomatic; `(require ...)` is the older spelling
        // and means the same thing.
        let clause_name = head.keyword().or_else(|| head.symbol());
        if !clause_name.is_some_and(|name| LOADING_CLAUSES.contains(&name)) {
            continue;
        }
        collect_libspecs(&parts[1..], &mut requires)?;
    }

    Ok(NsForm { name, requires })
}

/// Every grammar position that rejects a form reports it the same way: what the
/// position allows, then `describe()` of what was there instead.
fn wrong_form(form: &Form, expected: &str) -> ReadError {
    ReadError::new(form.offset, format!("{expected}, found {}", form.describe()))
}

/// A namespace name appears in four positions -- the `ns` name, a bare require,
/// a flat libspec's head, a prefix list's head -- and each one requires the
/// same thing of it: a symbol that is a well-formed namespace name.
fn namespace_symbol(form: &Form, expected: &str) -> Result<String, ReadError> {
    let name = form.symbol().ok_or_else(|| wrong_form(form, expected))?;
    validate_namespace_name(name, form.offset)?;
    Ok(name.to_owned())
}

fn validate_namespace_name(name: &str, offset: usize) -> Result<(), ReadError> {
    if name.contains('/') {
        return Err(ReadError::new(
            offset,
            format!("`{name}` is a qualified symbol, not a namespace name"),
        ));
    }
    if name.is_empty() || name.split('.').any(str::is_empty) {
        return Err(ReadError::new(
            offset,
            format!("`{name}` is not a well-formed namespace name"),
        ));
    }
    Ok(())
}

fn collect_libspecs(args: &[Form], out: &mut Vec<String>) -> Result<(), ReadError> {
    for arg in args {
        match &arg.kind {
            // Bare flags such as `:reload` / `:verbose` sit among the libspecs.
            Kind::Keyword(_) => {}
            Kind::Symbol(name) => {
                validate_namespace_name(name, arg.offset)?;
                out.push(name.clone());
            }
            Kind::Vector(items) => {
                if is_flat_libspec(items) {
                    if !has_as_alias(items) {
                        out.push(flat_libspec_name(arg, items)?);
                    }
                } else {
                    collect_prefix_list(arg, items, out)?;
                }
            }
            _ => return Err(wrong_form(arg, "a libspec must be a symbol or a vector")),
        }
    }
    Ok(())
}

/// `clojure.core/libspec?`: a vector is a plain libspec when its second element
/// is absent or a keyword option. Otherwise it is a prefix list.
fn is_flat_libspec(items: &[Form]) -> bool {
    items
        .get(1)
        .is_none_or(|second| matches!(second.kind, Kind::Keyword(_)))
}

/// `:as-alias` establishes an alias without loading the target namespace.
/// Such a libspec is deliberately absent from the unit graph: the target need
/// not exist, and adding an edge can create a cycle Clojure itself never loads.
fn has_as_alias(items: &[Form]) -> bool {
    items
        .iter()
        .skip(1)
        .step_by(2)
        .any(|item| item.keyword() == Some("as-alias"))
}

fn flat_libspec_name(spec: &Form, items: &[Form]) -> Result<String, ReadError> {
    let head = items
        .first()
        .ok_or_else(|| ReadError::new(spec.offset, "an empty vector is not a libspec"))?;
    namespace_symbol(head, "a libspec must name a namespace")
}

/// `[a.b [c :as x] [d] e]` means `a.b.c`, `a.b.d` and `a.b.e`. Clojure resolves
/// exactly one level of prefix, so a nested prefix list is an error there and
/// here.
fn collect_prefix_list(
    spec: &Form,
    items: &[Form],
    out: &mut Vec<String>,
) -> Result<(), ReadError> {
    let head = items
        .first()
        .ok_or_else(|| ReadError::new(spec.offset, "an empty vector is not a prefix list"))?;
    let prefix = namespace_symbol(head, "a prefix list must start with a symbol")?;

    for member in &items[1..] {
        let suffix = match &member.kind {
            Kind::Symbol(name) => {
                validate_namespace_name(name, member.offset)?;
                name.clone()
            }
            Kind::Vector(inner) if is_flat_libspec(inner) => {
                if has_as_alias(inner) {
                    continue;
                }
                flat_libspec_name(member, inner)?
            }
            Kind::Vector(_) => {
                return Err(ReadError::new(
                    member.offset,
                    "a prefix list cannot contain another prefix list",
                ));
            }
            _ => {
                return Err(wrong_form(
                    member,
                    "a prefix list member must be a symbol or a vector",
                ));
            }
        };
        out.push(format!("{prefix}.{suffix}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{NsForm, parse};

    fn parsed(src: &str) -> NsForm {
        parse(src).expect("ns form should parse")
    }

    #[test]
    fn plain_require_forms() {
        let ns = parsed("(ns foo.bar (:require [a.b :as x] [c.d :refer [e f]] g.h))");
        assert_eq!(ns.name, "foo.bar");
        assert_eq!(ns.requires, ["a.b", "c.d", "g.h"]);
    }

    #[test]
    fn prefix_lists_expand_against_their_prefix() {
        let ns = parsed("(ns foo.bar (:require [a.b [c :as x] [d] e]))");
        assert_eq!(ns.requires, ["a.b.c", "a.b.d", "a.b.e"]);
    }

    #[test]
    fn use_clauses_carry_the_same_edges_as_require() {
        let ns = parsed("(ns foo.bar (:use [a.b :only [x]] c.d))");
        assert_eq!(ns.requires, ["a.b", "c.d"]);
    }

    #[test]
    fn as_alias_libspecs_do_not_load_namespaces() {
        let ns = parsed(
            "(ns foo.bar (:require [app.types :as-alias types] [real.dep :as dep] [prefix [types :as-alias t] [loaded :as l]]))",
        );
        assert_eq!(ns.requires, ["real.dep", "prefix.loaded"]);
    }

    #[test]
    fn docstring_and_metadata_between_ns_and_require_are_skipped() {
        let ns = parsed(
            r#"(ns ^{:author "someone"} foo.bar
                 "A docstring, which is not a require clause."
                 {:added "1.0"}
                 (:require [a.b :as x]))"#,
        );
        assert_eq!(ns.name, "foo.bar");
        assert_eq!(ns.requires, ["a.b"]);
    }

    #[test]
    fn comments_inside_the_ns_form_are_ignored() {
        let ns = parsed(
            "(ns foo.bar\n  ;; why we need these\n  (:require [a.b :as x] ;; trailing note\n            [c.d]))",
        );
        assert_eq!(ns.requires, ["a.b", "c.d"]);
    }

    #[test]
    fn reader_conditionals_pick_the_jvm_branch() {
        let ns = parsed(
            "(ns foo.bar (:require #?(:clj [a.b :as x] :cljs [z.z :as x]) #?@(:clj [[c.d]] :cljs [[y.y]])))",
        );
        assert_eq!(ns.requires, ["a.b", "c.d"]);
    }

    #[test]
    fn import_and_gen_class_contribute_no_edges() {
        let ns = parsed(
            "(ns foo.bar (:require [a.b]) (:import [java.time Instant]) (:refer-clojure :exclude [get]) (:gen-class))",
        );
        assert_eq!(ns.requires, ["a.b"]);
    }

    #[test]
    fn require_flags_are_not_namespaces() {
        let ns = parsed("(ns foo.bar (:require [a.b :as x] :reload))");
        assert_eq!(ns.requires, ["a.b"]);
    }

    #[test]
    fn a_leading_shebang_and_comment_do_not_hide_the_ns_form() {
        let ns = parsed("#!/usr/bin/env bb\n;; header\n(ns foo.bar (:require a.b))");
        assert_eq!(ns.name, "foo.bar");
        assert_eq!(ns.requires, ["a.b"]);
    }

    #[test]
    fn a_file_without_an_ns_form_is_an_error_at_end_of_input() {
        let src = "(println :hello)\n";
        let error = parse(src).expect_err("should fail");
        assert_eq!(error.offset, src.len());
        assert!(error.message.contains("no `ns` form"), "{}", error.message);
    }

    #[test]
    fn a_malformed_libspec_is_an_error_at_its_offset() {
        let src = "(ns foo.bar (:require {:a 1}))";
        let error = parse(src).expect_err("should fail");
        assert_eq!(error.offset, src.find('{').expect("map in fixture"));
        assert!(error.message.contains("libspec"), "{}", error.message);
    }

    #[test]
    fn a_nested_prefix_list_is_an_error() {
        let src = "(ns foo.bar (:require [a [b [c :as x]]]))";
        let error = parse(src).expect_err("should fail");
        assert!(
            error.message.contains("another prefix list"),
            "{}",
            error.message
        );
    }
}
