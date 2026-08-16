use clippy_utils::diagnostics::span_lint_and_then;
use rustc_ast::ast;
use rustc_lint::{EarlyContext, EarlyLintPass, LintContext};
use rustc_session::impl_lint_pass;
use rustc_span::def_id::LOCAL_CRATE;
use rustc_span::FileName;
use std::path::Path;

declare_clippy_lint! {
    /// ### What it does
    /// Denies module filenames that contain underscores (e.g., `compute_vm.rs`).
    ///
    /// ### Why restrict this?
    /// Compound module names like `tap_pool.rs` or `vm_config.rs` should use
    /// directory hierarchy instead: `tap/pool.rs`, `vm/config.rs`. This keeps
    /// the filesystem structure aligned with the module tree and makes
    /// navigation easier.
    ///
    /// ### Example
    /// ```text
    /// src/compute_vm.rs
    /// src/compute_vm/snix_addrs.rs
    /// ```
    /// Use instead:
    /// ```text
    /// src/compute/vm.rs
    /// src/compute/vm/snix/addrs.rs
    /// ```
    #[clippy::version = "1.86.0"]
    pub UNDERSCORE_IN_MODULE_FILENAME,
    restriction,
    "module filename contains underscores; use directory hierarchy instead"
}

pub struct UnderscoreInModuleFilename;

impl UnderscoreInModuleFilename {
    pub fn new() -> Self {
        Self
    }
}

impl_lint_pass!(UnderscoreInModuleFilename => [UNDERSCORE_IN_MODULE_FILENAME]);

impl EarlyLintPass for UnderscoreInModuleFilename {
    fn check_crate(&mut self, cx: &EarlyContext<'_>, _krate: &ast::Crate) {
        let source_map = cx.sess().source_map();
        let working_dir = source_map
            .working_dir()
            .local_path()
            .map(Path::to_path_buf);

        for file in source_map.files().iter() {
            if file.cnum != LOCAL_CRATE {
                continue;
            }

            let FileName::Real(ref name) = file.name else {
                continue;
            };

            let Some(path) = name.local_path() else {
                continue;
            };

            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }

            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };

            // Skip conventional Rust filenames.
            if matches!(stem, "lib" | "main" | "mod" | "build") {
                continue;
            }

            if !stem.contains('_') {
                continue;
            }

            let display_path = working_dir
                .as_ref()
                .and_then(|wd| path.strip_prefix(wd).ok())
                .unwrap_or(path);

            let span = rustc_span::Span::with_root_ctxt(file.start_pos, file.start_pos);

            span_lint_and_then(
                cx,
                UNDERSCORE_IN_MODULE_FILENAME,
                span,
                format!(
                    "module filename `{}` contains underscores; use directory hierarchy instead \
                     (e.g., `{}`)",
                    display_path.display(),
                    suggest_hierarchy(display_path, stem),
                ),
                |diag| {
                    diag.help(
                        "rename `foo_bar.rs` to `foo/bar.rs` and update `mod` declarations",
                    );
                },
            );
        }
    }
}

/// Suggest a directory hierarchy for an underscored filename.
/// `src/compute_vm.rs` → `src/compute/vm.rs`
fn suggest_hierarchy(path: &Path, stem: &str) -> String {
    let parent = path.parent().unwrap_or(Path::new(""));
    let parts: Vec<&str> = stem.splitn(2, '_').collect();
    if parts.len() == 2 {
        parent.join(parts[0]).join(format!("{}.rs", parts[1])).display().to_string()
    } else {
        path.display().to_string()
    }
}
