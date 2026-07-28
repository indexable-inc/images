//! `skill-lint`: a non-panicking replacement for the `skillsaw` Python linter.
//!
//! It recursively finds every `SKILL.md` under a path, parses each file's
//! frontmatter with a real YAML parser, and reports diagnostics. Unlike
//! skillsaw it never panics on bad input and it surfaces the precise YAML
//! parser error with a file line number.

mod fix;
mod lint;

use std::{fs, path::Path, path::PathBuf, process::ExitCode};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use ignore::WalkBuilder;

use crate::lint::{Diagnostic, Severity, lint_skill};

const SKILL_FILE_NAME: &str = "SKILL.md";

#[derive(Debug, Parser)]
#[command(version, about = "Lint and autofix SKILL.md files")]
struct Cli {
    #[command(subcommand)]
    command: Option<Subcommands>,

    /// Path to lint (file or directory). Defaults to the current directory.
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,
}

#[derive(Debug, Subcommand)]
enum Subcommands {
    /// Apply safe autofixes (insert missing `name`, normalize whitespace).
    Fix {
        /// Path to fix (file or directory). Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Format {
    Human,
    Json,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Subcommands::Fix { path }) => run_fix(&path),
        None => run_lint(&cli.path, cli.format),
    };

    match result {
        Ok(code) => code,
        Err(error) => {
            // Operational failures (unreadable path, etc.) are distinct from
            // lint findings; report them and exit non-zero.
            eprintln!("skill-lint: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run_lint(path: &Path, format: Format) -> Result<ExitCode> {
    let skills = find_skills(path)?;
    let mut diagnostics = Vec::new();
    for skill in &skills {
        let contents = read_skill(skill)?;
        diagnostics.extend(lint_skill(skill, &contents));
    }

    let errors = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warnings = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count();

    match format {
        Format::Json => print_json(&diagnostics)?,
        Format::Human => {
            for diagnostic in &diagnostics {
                println!("{}", diagnostic.render());
            }
            println!("Errors: {errors}  Warnings: {warnings}");
        }
    }

    // Exit non-zero only on errors; warnings and info never fail the gate.
    Ok(if errors > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn run_fix(path: &Path) -> Result<ExitCode> {
    let skills = find_skills(path)?;
    for skill in &skills {
        let contents = read_skill(skill)?;
        let outcome = fix::fix_skill(skill, &contents);
        if let Some(fixed) = outcome.contents {
            fs::write(skill, &fixed).with_context(|| format!("writing {}", skill.display()))?;
            for change in &outcome.changes {
                println!("fixed {}: {change}", skill.display());
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn print_json(diagnostics: &[Diagnostic]) -> Result<()> {
    let json = serde_json::to_string_pretty(diagnostics).context("serializing diagnostics")?;
    println!("{json}");
    Ok(())
}

/// Find every `SKILL.md` under `path`. A single `SKILL.md` file path is
/// accepted directly; a directory is walked gitignore-aware.
fn find_skills(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    let mut skills = Vec::new();
    // `WalkBuilder` respects .gitignore, global ignores, and skips hidden dirs
    // by default. Directories without a SKILL.md (submodule containers, source
    // trees) are simply walked past since we only collect matching files.
    // Deliberate scope: we lint only tracked, non-hidden SKILL.md files
    // (gitignore-respecting). All of index's skills live under tracked,
    // non-hidden `skills/`, so an ignored/hidden SKILL.md is intentionally
    // skipped, not an oversight.
    for entry in WalkBuilder::new(path).build() {
        let entry = entry.context("walking the skills tree")?;
        if entry.file_name() == SKILL_FILE_NAME && entry.path().is_file() {
            skills.push(entry.path().to_path_buf());
        }
    }
    skills.sort();
    Ok(skills)
}

fn read_skill(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}
