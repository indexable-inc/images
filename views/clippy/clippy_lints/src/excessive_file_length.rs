use clippy_config::Conf;
use clippy_utils::diagnostics::span_lint_and_then;
use rustc_lint::{EarlyContext, EarlyLintPass, LintContext};
use rustc_session::impl_lint_pass;
use rustc_span::def_id::LOCAL_CRATE;
use rustc_span::FileName;
use std::path::Path;

declare_clippy_lint! {
    /// ### What it does
    /// Checks that source files do not exceed a configurable number of lines
    /// (default: 300).
    ///
    /// ### Why restrict this?
    /// Large files are harder to navigate, understand, and review. Splitting
    /// into smaller, focused modules improves maintainability.
    ///
    /// ### Example
    /// A 500-line file covering parsing, validation, and serialization.
    ///
    /// Split into:
    /// ```text
    /// parse.rs      (~120 lines)
    /// validate.rs   (~100 lines)
    /// serialize.rs  (~80 lines)
    /// mod.rs        (re-exports)
    /// ```
    #[clippy::version = "1.86.0"]
    pub EXCESSIVE_FILE_LENGTH,
    restriction,
    "source file exceeds the configured line count threshold"
}

pub struct ExcessiveFileLength {
    max_file_lines: u64,
}

impl ExcessiveFileLength {
    pub fn new(conf: &'static Conf) -> Self {
        Self {
            max_file_lines: conf.max_file_lines,
        }
    }
}

impl_lint_pass!(ExcessiveFileLength => [EXCESSIVE_FILE_LENGTH]);

impl EarlyLintPass for ExcessiveFileLength {
    fn check_crate(&mut self, cx: &EarlyContext<'_>, _krate: &rustc_ast::Crate) {
        if self.max_file_lines == 0 {
            return;
        }

        let source_map = cx.sess().source_map();
        let working_dir = source_map.working_dir().local_path().map(Path::to_path_buf);

        source_map.files().iter().for_each(|file| {
            if file.cnum != LOCAL_CRATE {
                return;
            }

            let FileName::Real(ref name) = file.name else {
                return;
            };

            let Some(path) = name.local_path() else {
                return;
            };

            // Only check .rs files
            if path.extension().is_some_and(|ext| ext != "rs") {
                return;
            }

            let line_count = file.lines().len() as u64;
            if line_count <= self.max_file_lines {
                return;
            }

            let display_path = working_dir
                .as_ref()
                .and_then(|wd| path.strip_prefix(wd).ok())
                .unwrap_or(path);

            // Use the span of the first byte in the file
            let span = rustc_span::Span::with_root_ctxt(file.start_pos, file.start_pos);

            span_lint_and_then(
                cx,
                EXCESSIVE_FILE_LENGTH,
                span,
                format!(
                    "file `{}` has {line_count} lines (max {})",
                    display_path.display(),
                    self.max_file_lines,
                ),
                |diag| {
                    diag.help("split into smaller, focused modules");
                },
            );
        });
    }
}
