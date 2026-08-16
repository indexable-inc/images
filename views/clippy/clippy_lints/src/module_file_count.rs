use clippy_config::Conf;
use clippy_utils::diagnostics::span_lint_and_then;
use rustc_ast::ast::{self, Inline, ItemKind, ModKind};
use rustc_lint::{EarlyContext, EarlyLintPass, LintContext};
use rustc_session::impl_lint_pass;
use rustc_span::def_id::LOCAL_CRATE;
use rustc_span::{FileName, SourceFile};
use std::path::{Path, PathBuf};

declare_clippy_lint! {
    /// ### What it does
    /// Checks that module directories do not contain more than a configurable
    /// number of `.rs` files (default: 7).
    ///
    /// ### Why restrict this?
    /// Large modules with many files are hard to navigate and signal missing
    /// abstraction boundaries. Splitting into sub-modules keeps each directory
    /// focused and discoverable.
    ///
    /// ### Example
    /// ```text
    /// src/handlers/
    ///   mod.rs a.rs b.rs c.rs d.rs e.rs f.rs g.rs h.rs  // 9 files
    /// ```
    /// Use instead:
    /// ```text
    /// src/handlers/
    ///   mod.rs
    ///   network/
    ///     a.rs b.rs c.rs
    ///   storage/
    ///     d.rs e.rs
    /// ```
    #[clippy::version = "1.86.0"]
    pub MODULE_FILE_COUNT,
    restriction,
    "module directory contains too many files"
}

pub struct ModuleFileCount {
    max_module_files: u64,
}

impl ModuleFileCount {
    pub fn new(conf: &'static Conf) -> Self {
        Self {
            max_module_files: conf.max_module_files,
        }
    }
}

impl_lint_pass!(ModuleFileCount => [MODULE_FILE_COUNT]);

impl EarlyLintPass for ModuleFileCount {
    fn check_item(&mut self, cx: &EarlyContext<'_>, item: &ast::Item) {
        if let ItemKind::Mod(.., ModKind::Loaded(_, Inline::No { .. }, mod_spans, ..)) = &item.kind
        {
            let mod_file = cx
                .sess()
                .source_map()
                .lookup_source_file(mod_spans.inner_span.lo());

            let Some(mod_dir) = module_directory(&mod_file) else {
                return;
            };

            let Ok(entries) = std::fs::read_dir(&mod_dir) else {
                return;
            };

            let rs_count = entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path().extension().is_some_and(|ext| ext == "rs")
                        && e.file_type().is_ok_and(|ft| ft.is_file())
                })
                .count() as u64;

            if rs_count > self.max_module_files {
                let working_dir = cx
                    .sess()
                    .source_map()
                    .working_dir()
                    .local_path()
                    .map(Path::to_path_buf);

                let display_dir = working_dir
                    .as_ref()
                    .and_then(|wd| mod_dir.strip_prefix(wd).ok())
                    .map_or_else(|| mod_dir.clone(), Path::to_path_buf);

                span_lint_and_then(
                    cx,
                    MODULE_FILE_COUNT,
                    item.span,
                    format!(
                        "module `{}` has {rs_count} files (max {})",
                        display_dir.display(),
                        self.max_module_files,
                    ),
                    |diag| {
                        diag.help("split into sub-modules to keep each directory focused");
                    },
                );
            }
        }
    }
}

fn module_directory(file: &SourceFile) -> Option<PathBuf> {
    let FileName::Real(ref name) = file.name else {
        return None;
    };
    let path = name.local_path()?;

    if file.cnum != LOCAL_CRATE {
        return None;
    }

    if path.ends_with("mod.rs") {
        path.parent().map(Path::to_path_buf)
    } else {
        // Self-named module: foo.rs -> foo/ directory
        Some(path.with_extension(""))
    }
}
