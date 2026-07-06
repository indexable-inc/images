//! `efx`: plan, apply, and report over `.efx` files.
//!
//! - `efx plan site.efx` — compile and diff against the journal, print
//!   per-effect verdicts, execute nothing.
//! - `efx apply site.efx` — execute what the diff demands, record results.
//! - `efx report --html out.html` — render the journal's run history as a
//!   self-contained HTML page.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use efx_engine::{Action, Journal, Verdict};
use efx_ir::Plan;

mod executors;
mod report;

#[derive(Parser)]
#[command(name = "efx", about = "Content-addressed effect engine", version)]
struct Cli {
    /// Journal file: the effect cache and run history.
    #[arg(long, global = true, default_value = "efx.journal.json")]
    journal: PathBuf,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show what an apply would do, without executing anything.
    Plan { file: PathBuf },
    /// Execute the plan and record results in the journal.
    Apply { file: PathBuf },
    /// Render the journal's run history to a self-contained HTML file.
    Report {
        #[arg(long)]
        html: PathBuf,
    },
}

fn load_plan(file: &PathBuf) -> Result<Plan> {
    let source =
        std::fs::read_to_string(file).with_context(|| format!("read {}", file.display()))?;
    efx_lang::compile(&source)
        .map_err(|err| anyhow::anyhow!("{}: {}", file.display(), err.render(&source)))
}

fn cmd_plan(file: &PathBuf, journal_path: &PathBuf) -> Result<ExitCode> {
    let plan = load_plan(file)?;
    let journal = Journal::load(journal_path)?;
    let report = efx_engine::plan(&plan, &journal)?;
    let executes = report
        .decisions
        .iter()
        .filter(|d| d.verdict == Verdict::Execute)
        .count();
    println!(
        "plan: {} effect(s), {} to execute, {} cached",
        report.decisions.len(),
        executes,
        report.decisions.len() - executes
    );
    for decision in &report.decisions {
        let verdict = match decision.verdict {
            Verdict::Cached => "cached ",
            Verdict::Execute => "execute",
        };
        println!(
            "  {verdict}  {} ({})  {}  [{}]",
            decision.name,
            decision.kind,
            decision.reason,
            decision.id.short()
        );
    }
    for orphan in &report.orphans {
        println!(
            "  orphan   {} ({})  journal entry no longer in the plan  [{}]",
            orphan.name,
            orphan.kind,
            &orphan.id[..12.min(orphan.id.len())]
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_apply(file: &PathBuf, journal_path: &PathBuf) -> Result<ExitCode> {
    let plan = load_plan(file)?;
    let mut journal = Journal::load(journal_path)?;
    let registry = executors::builtin_registry();
    let report = efx_engine::apply(&plan, &mut journal, &registry)?;
    for effect in &report.effects {
        let (verb, detail) = match effect.action {
            Action::Executed => ("executed", format!("{}ms", effect.duration_ms)),
            Action::Cached => ("cached  ", "cache hit".to_owned()),
            Action::Failed => ("failed  ", effect.reason.clone().unwrap_or_default()),
            Action::Skipped => ("skipped ", effect.reason.clone().unwrap_or_default()),
        };
        println!(
            "  {verb}  {} ({})  {detail}  [{}]",
            effect.name,
            effect.kind,
            effect.id.short()
        );
    }
    println!(
        "apply: {} executed, {} cached, {} failed, {} skipped",
        report.count(Action::Executed),
        report.count(Action::Cached),
        report.count(Action::Failed),
        report.count(Action::Skipped)
    );
    if report.succeeded() {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

fn cmd_report(journal_path: &PathBuf, html: &PathBuf) -> Result<ExitCode> {
    let journal = Journal::load(journal_path)?;
    let page = report::render(&journal.state);
    std::fs::write(html, page).with_context(|| format!("write {}", html.display()))?;
    println!(
        "report: {} run(s) -> {}",
        journal.state.runs.len(),
        html.display()
    );
    Ok(ExitCode::SUCCESS)
}

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    match &cli.command {
        Cmd::Plan { file } => cmd_plan(file, &cli.journal),
        Cmd::Apply { file } => cmd_apply(file, &cli.journal),
        Cmd::Report { html } => cmd_report(&cli.journal, html),
    }
}
