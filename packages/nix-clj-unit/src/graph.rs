//! Walk source roots, parse every `ns` form, and assemble the unit graph.
//!
//! Every check in here fails the whole render rather than degrading one
//! namespace, because the Nix side turns this JSON straight into derivations:
//! a namespace that quietly lost its edges builds against an incomplete
//! classpath and fails much later, in a build log, pointing at the wrong thing.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use color_eyre::eyre::{Result, bail, eyre};

use crate::model::{Graph, Namespace, SCHEMA_VERSION};
use crate::naming::{SOURCE_EXTENSIONS, namespace_to_relative_path, relative_path_to_namespace};
use crate::ns_form;

/// One `.clj`/`.cljc` file found under a source root.
struct Source {
    /// Path under the root, e.g. `com/example/todo_app/model/todo.clj`.
    relative: PathBuf,
    /// The root as written on the command line, joined with `relative`. This is
    /// both the path the file is read from and the path reported in the graph,
    /// so what the Nix side is told is what was actually parsed.
    path: PathBuf,
    /// `path` rendered once, for the graph and for error messages.
    display_path: String,
}

/// A parsed source: its declared namespace and its raw (unfiltered) requires.
struct Unit {
    source: Source,
    namespace: String,
    requires: Vec<String>,
}

struct LineColumn {
    line: usize,
    column: usize,
}

pub fn render(roots: &[PathBuf]) -> Result<Graph> {
    let units = parse_all(roots)?;
    let by_namespace = index_by_namespace(&units)?;

    let mut namespaces = BTreeMap::new();
    for unit in &units {
        let requires = internal_requires(unit, &by_namespace);
        namespaces.insert(
            unit.namespace.clone(),
            Namespace {
                file: unit.source.display_path.clone(),
                requires,
            },
        );
    }

    if let Some(cycle) = find_cycle(&namespaces) {
        bail!(
            "circular requires are illegal in Clojure, so this is a bug in the graph, \
             not in the sources: {}",
            cycle.join(" -> ")
        );
    }

    Ok(Graph {
        version: SCHEMA_VERSION,
        namespaces,
    })
}

fn parse_all(roots: &[PathBuf]) -> Result<Vec<Unit>> {
    let mut units = Vec::new();
    for root in roots {
        if !root.is_dir() {
            bail!("source root `{}` is not a directory", root.display());
        }
        for relative in collect_sources(root)? {
            let path = root.join(&relative);
            let display_path = path.display().to_string();
            units.push(parse_source(Source {
                relative,
                path,
                display_path,
            })?);
        }
    }
    Ok(units)
}

fn parse_source(source: Source) -> Result<Unit> {
    let text = fs::read_to_string(&source.path)
        .map_err(|error| eyre!("{}: cannot read: {error}", source.display_path))?;

    let ns = ns_form::parse(&text).map_err(|error| {
        let at = position(&text, error.offset);
        eyre!(
            "{}: cannot parse the `ns` form at byte offset {} (line {}, column {}): {}",
            source.display_path,
            error.offset,
            at.line,
            at.column,
            error.message
        )
    })?;

    // Clojure locates a namespace purely by munging its name into a path, so a
    // file whose `ns` disagrees with where it sits is unloadable however well
    // it parses.
    let extension = source
        .relative
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    let expected = namespace_to_relative_path(&ns.name, extension);
    if expected != source.relative {
        let implied = relative_path_to_namespace(&source.relative)
            .unwrap_or_else(|| "an unrepresentable namespace".to_owned());
        bail!(
            "{}: declares namespace `{}`, which Clojure loads from `{}`; \
             this file's path declares `{}` instead",
            source.display_path,
            ns.name,
            expected.display(),
            implied
        );
    }

    Ok(Unit {
        source,
        namespace: ns.name,
        requires: ns.requires,
    })
}

fn index_by_namespace(units: &[Unit]) -> Result<BTreeMap<&str, &Unit>> {
    let mut index: BTreeMap<&str, &Unit> = BTreeMap::new();
    for unit in units {
        if let Some(previous) = index.insert(&unit.namespace, unit) {
            bail!(
                "namespace `{}` is declared twice: {} and {}",
                unit.namespace,
                previous.source.display_path,
                unit.source.display_path
            );
        }
    }
    Ok(index)
}

/// Keep only requires whose namespaces are present in this graph.
///
/// Graph membership is the ownership boundary. A namespace absent from the
/// source roots may legitimately come from a jar or git dependency even when
/// its name shares every segment of an application namespace, so a common
/// string prefix cannot classify it as a missing source file. The JVM remains
/// the authority for reporting a truly missing external namespace during the
/// unit's `require` step.
fn internal_requires(unit: &Unit, by_namespace: &BTreeMap<&str, &Unit>) -> Vec<String> {
    unit.requires
        .iter()
        .filter(|required| by_namespace.contains_key(required.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn find_cycle(namespaces: &BTreeMap<String, Namespace>) -> Option<Vec<String>> {
    let mut settled = BTreeSet::new();
    let mut stack = Vec::new();
    for start in namespaces.keys() {
        if let Some(cycle) = visit(start, namespaces, &mut settled, &mut stack) {
            return Some(cycle);
        }
    }
    None
}

/// Depth-first search whose recursion depth is bounded by the length of the
/// longest require chain, which Clojure itself has to be able to load.
fn visit(
    namespace: &str,
    namespaces: &BTreeMap<String, Namespace>,
    settled: &mut BTreeSet<String>,
    stack: &mut Vec<String>,
) -> Option<Vec<String>> {
    if settled.contains(namespace) {
        return None;
    }
    if let Some(entered) = stack.iter().position(|seen| seen == namespace) {
        let mut cycle = stack[entered..].to_vec();
        cycle.push(namespace.to_owned());
        return Some(cycle);
    }

    stack.push(namespace.to_owned());
    for required in &namespaces[namespace].requires {
        if let Some(cycle) = visit(required, namespaces, settled, stack) {
            return Some(cycle);
        }
    }
    stack.pop();
    settled.insert(namespace.to_owned());
    None
}

/// Source paths under `root`, relative to it, in a stable order.
fn collect_sources(root: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    walk(root, Path::new(""), &mut found)?;
    found.sort();
    Ok(found)
}

fn walk(directory: &Path, prefix: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(directory)
        .map_err(|error| eyre!("cannot read directory `{}`: {error}", directory.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|error| eyre!("cannot read directory `{}`: {error}", directory.display()))?;
    entries.sort_by_key(fs::DirEntry::file_name);

    for entry in entries {
        let name = entry.file_name();
        let relative = prefix.join(&name);
        // `file_type` does not follow symlinks, so a symlinked directory is not
        // descended into and a link loop cannot hang the walk.
        let file_type = entry
            .file_type()
            .map_err(|error| eyre!("cannot stat `{}`: {error}", relative.display()))?;
        if file_type.is_dir() {
            walk(&directory.join(&name), &relative, found)?;
        } else if is_source(&relative) {
            found.push(relative);
        }
    }
    Ok(())
}

fn is_source(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| SOURCE_EXTENSIONS.contains(&extension))
}

fn position(text: &str, offset: usize) -> LineColumn {
    let clamped = offset.min(text.len());
    let consumed = &text[..clamped];
    let line = consumed.matches('\n').count() + 1;
    let column = consumed
        .rfind('\n')
        .map_or(clamped, |newline| clamped - newline - 1)
        + 1;
    LineColumn { line, column }
}

#[cfg(test)]
mod tests {
    use super::render;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// Materialise `(relative path, contents)` pairs into a temporary source
    /// root and render it.
    struct Tree {
        directory: tempfile::TempDir,
    }

    impl Tree {
        fn new(files: &[(&str, &str)]) -> Self {
            let directory = tempfile::tempdir().expect("temp dir");
            for (relative, contents) in files {
                let path = directory.path().join(relative);
                std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
                std::fs::write(&path, contents).expect("write");
            }
            Self { directory }
        }

        fn root(&self) -> PathBuf {
            self.directory.path().to_path_buf()
        }

        fn edges(&self) -> BTreeMap<String, Vec<String>> {
            render(&[self.root()])
                .expect("tree should render")
                .namespaces
                .into_iter()
                .map(|(name, entry)| (name, entry.requires))
                .collect()
        }

        fn failure(&self) -> String {
            format!("{:#}", render(&[self.root()]).expect_err("should fail"))
        }
    }

    #[test]
    fn external_requires_are_dropped_and_internal_ones_kept() {
        let tree = Tree::new(&[
            (
                "app/core.clj",
                "(ns app.core (:require [clojure.string :as str] [app.util :as util]))",
            ),
            (
                "app/util.clj",
                "(ns app.util (:require [ring.util.response :as r]))",
            ),
        ]);
        assert_eq!(
            tree.edges(),
            BTreeMap::from([
                ("app.core".to_owned(), vec!["app.util".to_owned()]),
                ("app.util".to_owned(), Vec::new()),
            ])
        );
    }

    #[test]
    fn hyphenated_namespaces_are_found_under_underscored_paths() {
        let tree = Tree::new(&[
            (
                "com/example/todo_app/model/todo.clj",
                "(ns com.example.todo-app.model.todo (:require [com.example.todo-app.model.tab-state :as t]))",
            ),
            (
                "com/example/todo_app/model/tab_state.clj",
                "(ns com.example.todo-app.model.tab-state)",
            ),
        ]);
        assert_eq!(
            tree.edges()["com.example.todo-app.model.todo"],
            ["com.example.todo-app.model.tab-state"]
        );
    }

    #[test]
    fn cljc_sources_are_units_too() {
        let tree = Tree::new(&[
            (
                "app/core.cljc",
                "(ns app.core (:require #?(:clj [app.jvm :as j] :cljs [app.browser :as b])))",
            ),
            ("app/jvm.cljc", "(ns app.jvm)"),
        ]);
        assert_eq!(tree.edges()["app.core"], ["app.jvm"]);
    }

    #[test]
    fn requires_are_sorted_and_deduplicated() {
        let tree = Tree::new(&[
            (
                "app/core.clj",
                "(ns app.core (:require [app.z] [app.a] [app.z :as z2]))",
            ),
            ("app/a.clj", "(ns app.a)"),
            ("app/z.clj", "(ns app.z)"),
        ]);
        assert_eq!(tree.edges()["app.core"], ["app.a", "app.z"]);
    }

    #[test]
    fn file_paths_carry_the_source_root_as_written() {
        let tree = Tree::new(&[("app/core.clj", "(ns app.core)")]);
        let root = tree.root();
        let graph = render(std::slice::from_ref(&root)).expect("render");
        assert_eq!(
            graph.namespaces["app.core"].file,
            root.join("app/core.clj").display().to_string()
        );
    }

    #[test]
    fn an_external_namespace_may_share_the_source_prefix() {
        let tree = Tree::new(&[(
            "foo/core.clj",
            "(ns foo.core (:require [foo.core.plugin :as plugin]))",
        )]);
        assert_eq!(tree.edges()["foo.core"], Vec::<String>::new());
    }

    #[test]
    fn a_cycle_is_an_error_printing_the_cycle() {
        let tree = Tree::new(&[
            ("app/a.clj", "(ns app.a (:require [app.b]))"),
            ("app/b.clj", "(ns app.b (:require [app.a]))"),
        ]);
        let failure = tree.failure();
        assert!(failure.contains("app.a -> app.b -> app.a"), "{failure}");
    }

    #[test]
    fn an_unparseable_ns_form_names_the_file_and_the_offset() {
        let tree = Tree::new(&[("app/core.clj", "(ns app.core (:require [app.util)")]);
        let failure = tree.failure();
        assert!(failure.contains("app/core.clj"), "{failure}");
        assert!(failure.contains("byte offset 32"), "{failure}");
    }

    #[test]
    fn a_namespace_that_disagrees_with_its_path_is_an_error() {
        let tree = Tree::new(&[("app/core.clj", "(ns app.kernel)")]);
        let failure = tree.failure();
        assert!(failure.contains("app/kernel.clj"), "{failure}");
        assert!(failure.contains("app.core"), "{failure}");
    }

    #[test]
    fn the_same_namespace_under_two_roots_is_an_error() {
        let first = Tree::new(&[("app/core.clj", "(ns app.core)")]);
        let second = Tree::new(&[("app/core.clj", "(ns app.core)")]);
        let failure = format!(
            "{:#}",
            render(&[first.root(), second.root()]).expect_err("should fail")
        );
        assert!(failure.contains("declared twice"), "{failure}");
    }

    #[test]
    fn two_roots_are_merged_into_one_graph() {
        let library = Tree::new(&[("lib/core.clj", "(ns lib.core)")]);
        let app = Tree::new(&[("app/core.clj", "(ns app.core (:require [lib.core]))")]);
        let graph = render(&[library.root(), app.root()]).expect("render");
        assert_eq!(graph.namespaces["app.core"].requires, ["lib.core"]);
    }

    #[test]
    fn the_json_is_byte_identical_across_runs() {
        let tree = Tree::new(&[
            ("app/core.clj", "(ns app.core (:require [app.z] [app.a]))"),
            ("app/a.clj", "(ns app.a)"),
            ("app/z.clj", "(ns app.z (:require [app.a]))"),
        ]);
        let first = serde_json::to_string_pretty(&render(&[tree.root()]).expect("render"))
            .expect("serialize");
        let second = serde_json::to_string_pretty(&render(&[tree.root()]).expect("render"))
            .expect("serialize");
        assert_eq!(first, second);
    }
}
