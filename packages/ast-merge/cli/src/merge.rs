use snafu::ResultExt as _;

use crate::{CliError, CliResult, ParseSnafu, ReadRevisionSnafu, WriteResultSnafu};

pub struct Params {
    pub base: std::path::PathBuf,
    pub left: std::path::PathBuf,
    pub right: std::path::PathBuf,
    pub output: Option<std::path::PathBuf>,
    pub language: Option<String>,
    pub git_mode: bool,
}

pub fn run(params: Params) -> CliResult {
    let Params {
        base,
        left,
        right,
        output,
        language,
        git_mode,
    } = params;
    let inner = || -> Result<bool, CliError> {
        tracing::info!(?base, ?left, ?right, "performing merge");

        let lang = resolve_language(language.as_deref(), &left)?;

        let base_content = ast_merge_git::read_revision(&base).context(ReadRevisionSnafu)?;
        let left_content = ast_merge_git::read_revision(&left).context(ReadRevisionSnafu)?;
        let right_content = ast_merge_git::read_revision(&right).context(ReadRevisionSnafu)?;

        let result = if let Some(lang) = lang {
            tracing::info!(language = lang.name(), "using AST-based merge");
            tree_based(&TreeInput {
                base: &base_content,
                left: &left_content,
                right: &right_content,
                lang,
            })?
        } else {
            tracing::info!("using line-based merge (language not detected)");
            ast_merge_diff::based(&base_content, &left_content, &right_content)
        };

        let output_path = if git_mode {
            left
        } else {
            output.unwrap_or(left)
        };

        ast_merge_git::write_result(&output_path, &result.content).context(WriteResultSnafu)?;

        if result.success {
            tracing::info!("merge successful");
            Ok(true)
        } else {
            tracing::warn!(
                conflicts = result.conflicts.len(),
                "merge completed with conflicts"
            );
            Ok(false)
        }
    };

    match inner() {
        Ok(true) => CliResult::Ok,
        Ok(false) => CliResult::Conflicts,
        Err(e) => CliResult::Err(e),
    }
}

pub fn detect_by_name(name: &str) -> Option<ast_merge_langs::Lang> {
    let name_lower = name.to_lowercase();
    for lang in ast_merge_langs::Lang::all() {
        if lang.name().to_lowercase() == name_lower {
            return Some(*lang);
        }
    }
    None
}

pub fn resolve_language(
    language: Option<&str>,
    left: &std::path::Path,
) -> Result<Option<ast_merge_langs::Lang>, CliError> {
    language.map_or_else(
        || Ok(ast_merge_langs::detect(left)),
        |name| {
            detect_by_name(name)
                .map(Some)
                .ok_or_else(|| CliError::UnknownLanguageName {
                    name: name.to_owned(),
                })
        },
    )
}

struct TreeInput<'a> {
    base: &'a str,
    left: &'a str,
    right: &'a str,
    lang: ast_merge_langs::Lang,
}

fn tree_based(input: &TreeInput<'_>) -> Result<ast_merge_diff::Result, CliError> {
    let ts_lang = input.lang.to_tree_sitter();

    let base_parsed = ast_merge_ast::tree(input.base, &ts_lang).context(ParseSnafu)?;
    let left_parsed = ast_merge_ast::tree(input.left, &ts_lang).context(ParseSnafu)?;
    let right_parsed = ast_merge_ast::tree(input.right, &ts_lang).context(ParseSnafu)?;

    if base_parsed.has_errors || left_parsed.has_errors || right_parsed.has_errors {
        tracing::warn!("parse errors detected, falling back to line-based merge");
        return Ok(ast_merge_diff::based(input.base, input.left, input.right));
    }

    let base_left_matching = ast_merge_matcher::compute(&base_parsed.tree, &left_parsed.tree);
    let base_right_matching = ast_merge_matcher::compute(&base_parsed.tree, &right_parsed.tree);

    tracing::debug!(
        base_left = base_left_matching.len(),
        base_right = base_right_matching.len(),
        "computed matchings"
    );

    let merger = ast_merge_diff::ThreeWay::new(ast_merge_diff::ThreeWayParams {
        trees: ast_merge_diff::ThreeWayTrees {
            base: &base_parsed.tree,
            left: &left_parsed.tree,
            right: &right_parsed.tree,
        },
        base_left_matching,
        base_right_matching,
        config: ast_merge_diff::Config::default(),
    });

    Ok(merger.merge())
}
