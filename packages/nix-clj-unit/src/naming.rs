//! The Clojure namespace <-> file path correspondence.
//!
//! `clojure.lang.RT` munges a namespace into a resource name by replacing `-`
//! with `_` and `.` with `/`, so `com.example.todo-app.model.tab-state` loads
//! from `com/example/todo_app/model/tab_state.clj`. Getting this wrong does not
//! fail loudly at build time -- it fails as a `ClassNotFoundException` inside a
//! unit -- so it lives in one place with tests on both directions.

use std::path::{Path, PathBuf};

/// Source extensions a unit can be compiled from. `.cljs` is absent on purpose:
/// ClojureScript is not AOT-compiled by `clojure.main`.
pub const SOURCE_EXTENSIONS: [&str; 2] = ["clj", "cljc"];

/// Where Clojure will look for `namespace`.
pub fn namespace_to_relative_path(namespace: &str, extension: &str) -> PathBuf {
    let mut path = PathBuf::new();
    for segment in namespace.split('.') {
        path.push(segment.replace('-', "_"));
    }
    path.set_extension(extension);
    path
}

/// The namespace a file at `relative` would be expected to declare.
///
/// This is only a best-effort inverse: `-` -> `_` is not injective, so a
/// namespace that really contains an underscore round-trips to a hyphen. It is
/// therefore used for diagnostics only; [`namespace_to_relative_path`] is the
/// authoritative direction and the one every lookup goes through.
pub fn relative_path_to_namespace(relative: &Path) -> Option<String> {
    let stem = relative.file_stem()?.to_str()?;
    let mut segments: Vec<String> = relative
        .parent()?
        .components()
        .map(|component| component.as_os_str().to_string_lossy().replace('_', "-"))
        .collect();
    segments.push(stem.replace('_', "-"));
    Some(segments.join("."))
}

#[cfg(test)]
mod tests {
    use super::{namespace_to_relative_path, relative_path_to_namespace};
    use std::path::Path;

    #[test]
    fn hyphens_become_underscores_and_dots_become_directories() {
        assert_eq!(
            namespace_to_relative_path("com.example.todo-app.model.tab-state", "clj"),
            Path::new("com/example/todo_app/model/tab_state.clj")
        );
    }

    #[test]
    fn a_single_segment_namespace_is_a_bare_file() {
        assert_eq!(
            namespace_to_relative_path("user", "cljc"),
            Path::new("user.cljc")
        );
    }

    #[test]
    fn underscores_become_hyphens_on_the_way_back() {
        assert_eq!(
            relative_path_to_namespace(Path::new("com/example/todo_app/model/tab_state.clj"))
                .as_deref(),
            Some("com.example.todo-app.model.tab-state")
        );
    }

    #[test]
    fn every_hyphenated_namespace_round_trips_through_its_path() {
        for namespace in [
            "com.example.todo-app",
            "com.example.todo-app.model.tab-state",
            "com.example.reading-list",
        ] {
            let path = namespace_to_relative_path(namespace, "clj");
            assert_eq!(
                relative_path_to_namespace(&path).as_deref(),
                Some(namespace),
                "round trip through {}",
                path.display()
            );
        }
    }
}
