mod merge;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use snafu::ResultExt as _;

const EXIT_CONFLICTS: u8 = 1;

#[derive(Parser)]
#[command(name = "ast-merge")]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Merge {
        #[arg(value_name = "BASE")]
        base: PathBuf,
        #[arg(value_name = "LEFT")]
        left: PathBuf,
        #[arg(value_name = "RIGHT")]
        right: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(short, long)]
        language: Option<String>,
        #[arg(long)]
        git: bool,
    },
    Solve {
        #[arg(value_name = "FILE")]
        file: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    Languages,
    Info,
}

#[derive(Debug, snafu::Snafu)]
#[snafu(visibility(pub(crate)))]
enum CliError {
    #[snafu(display("failed to read revision"))]
    ReadRevision {
        source: ast_merge_git::RevisionError,
    },
    #[snafu(display("failed to write merge result"))]
    WriteResult {
        source: ast_merge_git::RevisionError,
    },
    #[snafu(display("failed to parse for merge"))]
    Parse { source: ast_merge_ast::Error },
    #[snafu(display("cannot detect language for {path} - specify with --language"))]
    UnknownLanguage { path: String },
    #[snafu(display("unknown language {name}"))]
    UnknownLanguageName { name: String },
    #[snafu(display("write failed"))]
    Write { source: std::io::Error },
}

enum CliResult {
    Ok,
    Conflicts,
    Err(CliError),
}

impl From<Result<(), CliError>> for CliResult {
    fn from(result: Result<(), CliError>) -> Self {
        match result {
            Ok(()) => Self::Ok,
            Err(e) => Self::Err(e),
        }
    }
}

/// Install a tracing subscriber reading filter directives from `RUST_LOG`
/// (defaulting to `info`) and writing formatted events to stderr. Replaces the
/// `ix` platform's `service_init::init`, which this standalone tool does not
/// depend on.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr))
        .init();
}

fn main() -> std::process::ExitCode {
    init_tracing();

    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Merge {
            base,
            left,
            right,
            output,
            language,
            git,
        } => merge::run(merge::Params {
            base,
            left,
            right,
            output,
            language,
            git_mode: git,
        }),
        Commands::Solve { file, output } => cmd_solve(file, output).into(),
        Commands::Languages => cmd_languages().into(),
        Commands::Info => cmd_info().into(),
    };

    match result {
        CliResult::Ok => std::process::ExitCode::SUCCESS,
        CliResult::Conflicts => std::process::ExitCode::from(EXIT_CONFLICTS),
        CliResult::Err(e) => {
            tracing::error!("error: {e:?}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn cmd_solve(file: PathBuf, output: Option<PathBuf>) -> Result<(), CliError> {
    tracing::info!(?file, "attempting to solve conflicts");

    let content = ast_merge_git::read_revision(&file).context(ReadRevisionSnafu)?;
    let parsed = ast_merge_git::conflicts(&content);

    if !parsed.has_conflicts {
        tracing::info!("no conflicts found");
        return Ok(());
    }

    tracing::info!(count = parsed.conflicts.len(), "found conflicts");

    let lang = ast_merge_langs::detect(&file);
    if lang.is_none() {
        return Err(CliError::UnknownLanguage {
            path: file.display().to_string(),
        });
    }

    tracing::warn!("conflict solving not yet fully implemented");

    let output_path = output.unwrap_or(file);
    ast_merge_git::write_result(&output_path, &content).context(WriteResultSnafu)?;

    Ok(())
}

fn cmd_languages() -> Result<(), CliError> {
    use std::io::Write as _;

    let stdout = std::io::stdout();
    let mut handle = stdout.lock();

    writeln!(handle, "Supported languages:").context(WriteSnafu)?;
    writeln!(handle).context(WriteSnafu)?;

    for lang in ast_merge_langs::Lang::all() {
        let profile = lang.profile();
        writeln!(handle, "  {}", profile.name).context(WriteSnafu)?;
        writeln!(handle, "    Extensions: {}", profile.extensions.join(", "))
            .context(WriteSnafu)?;
        if !profile.file_names.is_empty() {
            writeln!(handle, "    Files: {}", profile.file_names.join(", ")).context(WriteSnafu)?;
        }
        writeln!(handle).context(WriteSnafu)?;
    }

    Ok(())
}

fn cmd_info() -> Result<(), CliError> {
    use std::io::Write as _;

    let stdout = std::io::stdout();
    let mut handle = stdout.lock();

    writeln!(handle, "ast-merge {}", env!("CARGO_PKG_VERSION")).context(WriteSnafu)?;
    writeln!(handle).context(WriteSnafu)?;
    writeln!(handle, "AST-aware git merge driver using tree-sitter").context(WriteSnafu)?;
    writeln!(handle).context(WriteSnafu)?;
    writeln!(handle, "Git merge driver configuration:").context(WriteSnafu)?;
    writeln!(handle).context(WriteSnafu)?;
    writeln!(handle, "  1. Add to .gitattributes:").context(WriteSnafu)?;
    writeln!(handle, "     *.rs merge=ast-merge").context(WriteSnafu)?;
    writeln!(handle, "     *.ts merge=ast-merge").context(WriteSnafu)?;
    writeln!(handle).context(WriteSnafu)?;
    writeln!(handle, "  2. Configure git:").context(WriteSnafu)?;
    writeln!(
        handle,
        "     git config merge.ast-merge.driver 'ast-merge merge %O %A %B --git'"
    )
    .context(WriteSnafu)?;
    writeln!(handle).context(WriteSnafu)?;
    writeln!(
        handle,
        "Languages supported: {}",
        ast_merge_langs::Lang::all().len()
    )
    .context(WriteSnafu)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn detect_by_name() {
        assert!(merge::detect_by_name("rust").is_some());
        assert!(merge::detect_by_name("Rust").is_some());
        assert!(merge::detect_by_name("RUST").is_some());
        assert!(merge::detect_by_name("typescript").is_some());
        assert!(merge::detect_by_name("unknown").is_none());
    }

    #[test]
    fn resolve_language_rejects_unknown() {
        let result =
            merge::resolve_language(Some("not-a-language"), std::path::Path::new("main.rs"));
        assert!(matches!(
            result,
            Err(CliError::UnknownLanguageName { ref name }) if name == "not-a-language"
        ));
    }

    #[test]
    fn run_merge_rejects_unknown_language() {
        let dir = test_dir();
        let base = dir.join("base.rs");
        let left = dir.join("left.rs");
        let right = dir.join("right.rs");
        let output = dir.join("merged.rs");
        let content = "fn main() {}\n";

        std::fs::write(&base, content).expect("write base");
        std::fs::write(&left, content).expect("write left");
        std::fs::write(&right, content).expect("write right");

        let result = merge::run(merge::Params {
            base,
            left,
            right,
            output: Some(output.clone()),
            language: Some(String::from("not-a-language")),
            git_mode: false,
        });

        assert!(matches!(
            result,
            CliResult::Err(CliError::UnknownLanguageName { ref name }) if name == "not-a-language"
        ));
        assert!(!output.exists());
    }

    fn test_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ast-merge-cli-tests-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp test dir");
        dir
    }
}
