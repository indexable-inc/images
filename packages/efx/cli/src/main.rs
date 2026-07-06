//! `efx`: plan, apply, and report over `.efx` files or plan IR JSON.
//!
//! - `efx plan site.efx` — compile and diff against the journal, print
//!   per-effect verdicts, execute nothing.
//! - `efx plan --ir plan.json` — same, over a plan IR document produced by
//!   another frontend (the nix efx library, or any program emitting
//!   `efx_ir::Plan` JSON).
//! - `efx apply site.efx` / `efx apply --ir plan.json` — execute what the
//!   diff demands, record results.
//! - `efx report --html out.html` — render the journal's run history as a
//!   self-contained HTML page.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use efx_engine::{Action, Journal, Verdict};
use efx_ir::Plan;

mod cloudflare;
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
    Plan {
        file: PathBuf,
        /// Read FILE as plan IR JSON (an `efx_ir::Plan` document, as emitted
        /// by the nix efx library) instead of `.efx` source.
        #[arg(long)]
        ir: bool,
    },
    /// Execute the plan and record results in the journal.
    Apply {
        file: PathBuf,
        /// Read FILE as plan IR JSON (an `efx_ir::Plan` document, as emitted
        /// by the nix efx library) instead of `.efx` source.
        #[arg(long)]
        ir: bool,
    },
    /// Render the journal's run history to a self-contained HTML file.
    Report {
        #[arg(long)]
        html: PathBuf,
    },
}

fn load_plan(file: &PathBuf, ir: bool) -> Result<Plan> {
    let source =
        std::fs::read_to_string(file).with_context(|| format!("read {}", file.display()))?;
    if ir {
        return serde_json::from_str(&source)
            .with_context(|| format!("{}: not a valid plan IR document", file.display()));
    }
    efx_lang::compile(&source)
        .map_err(|err| anyhow::anyhow!("{}: {}", file.display(), err.render(&source)))
}

fn cmd_plan(file: &PathBuf, ir: bool, journal_path: &PathBuf) -> Result<ExitCode> {
    let plan = load_plan(file, ir)?;
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

fn cmd_apply(file: &PathBuf, ir: bool, journal_path: &PathBuf) -> Result<ExitCode> {
    let plan = load_plan(file, ir)?;
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
        Cmd::Plan { file, ir } => cmd_plan(file, *ir, &cli.journal),
        Cmd::Apply { file, ir } => cmd_apply(file, *ir, &cli.journal),
        Cmd::Report { html } => cmd_report(&cli.journal, html),
    }
}
