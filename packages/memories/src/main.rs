//! `memories`: search, lint and write the per-repo `.memories` corpus.
//!
//! Exit codes are the contract's: 0 on success, 1 on a lint error or a slug
//! that does not resolve, 2 on a usage error. `--json` output is specified;
//! human output is for a terminal and deliberately is not.

use anyhow::{Context as _, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use memories::{
    discover::{self, Corpus},
    lint,
    model::{self, Genre},
    report::{self, Scores},
    search::{self, Query},
    write::{self, RememberSpec},
};
use std::{io::Read as _, path::PathBuf, process::ExitCode, time::Instant};

/// Hits returned when the caller does not say. Ten is a screenful and a
/// prompt's worth; a caller wanting everything passes `--limit`.
const DEFAULT_LIMIT: usize = 10;

/// Exit code for a usage error, matching what clap itself returns.
const USAGE_EXIT_CODE: u8 = 2;

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Search, lint and write the per-repo `.memories` corpus"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// A `.memories` directory (or the directory containing one). Repeatable,
    /// and overrides the default root set entirely.
    #[arg(long = "dir", global = true, value_name = "PATH")]
    dirs: Vec<PathBuf>,

    /// Emit the JSON contract instead of terminal output.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// The `.memories` directories a search would read, resolved.
    Roots,
    /// Ranked search over every discovered memory.
    Search {
        query: String,
        #[arg(long, default_value_t = DEFAULT_LIMIT)]
        limit: usize,
        /// Keep only memories carrying one of these topics. Repeatable.
        #[arg(long)]
        topic: Vec<String>,
        /// Keep only these genres. Repeatable.
        #[arg(long, value_enum)]
        genre: Vec<Genre>,
        /// Include refuted memories, which are otherwise excluded.
        #[arg(long)]
        all: bool,
    },
    /// One memory by slug.
    Show { slug: String },
    /// Memories whose `based_on` no longer matches.
    Stale,
    /// Memories whose newest validation says they did not hold.
    Refuted,
    /// Memories nobody has validated lately.
    Unchecked {
        #[arg(long, default_value_t = report::default_unchecked_days())]
        days: f64,
    },
    /// Check every memory against the format's rules.
    Lint {
        /// Apply the unambiguous fixes: sort `topic`/`handle`, refresh
        /// `based_on` hashes, normalize whitespace.
        #[arg(long)]
        fix: bool,
    },
    /// Write a new memory. The body is read from stdin.
    Remember {
        slug: String,
        #[arg(long)]
        tldr: String,
        #[arg(long, value_enum, default_value_t = Genre::Memory)]
        genre: Genre,
        #[arg(long)]
        topic: Vec<String>,
        #[arg(long)]
        handle: Vec<String>,
        #[arg(long, default_value_t = model::DEFAULT_PRIOR)]
        prior: f64,
        #[arg(long)]
        related: Vec<String>,
        #[arg(long = "based-on")]
        based_on: Vec<String>,
        /// Who proved it. Required for `--genre memory`.
        #[arg(long)]
        by: Option<String>,
        /// The command that proved it, re-runnable. Required for `--genre memory`.
        #[arg(long)]
        how: Option<String>,
        /// `shared` (the default) or `user:<name>` for a memory that is one
        /// person's. Nothing is ever injected unasked, whatever the scope.
        #[arg(long, default_value = "shared")]
        scope: String,
    },
    /// Record that a memory still holds, refreshing its `based_on` hashes.
    Validate {
        slug: String,
        #[arg(long)]
        by: String,
        #[arg(long)]
        how: String,
        /// Record that it did not hold. Same as `refute` without `--instead`.
        #[arg(long = "not-ok")]
        not_ok: bool,
    },
    /// Record that a memory did not hold, optionally naming its successor.
    Refute {
        slug: String,
        #[arg(long)]
        by: String,
        #[arg(long)]
        how: String,
        /// The memory that replaces it. `supersedes` lives on the successor, so
        /// this writes to that file too.
        #[arg(long)]
        instead: Option<String>,
    },
}

fn main() -> ExitCode {
    cli_entry::run("memories", |cli: Cli| run(&cli))
}

fn run(cli: &Cli) -> Result<ExitCode> {
    match &cli.command {
        Command::Roots => run_roots(cli),
        Command::Search {
            query,
            limit,
            topic,
            genre,
            all,
        } => run_search(cli, query, *limit, topic, genre, *all),
        Command::Show { slug } => run_show(cli, slug),
        Command::Stale => run_stale(cli),
        Command::Refuted => run_refuted(cli),
        Command::Unchecked { days } => run_unchecked(cli, *days),
        Command::Lint { fix } => run_lint(cli, *fix),
        Command::Remember {
            slug,
            tldr,
            genre,
            topic,
            handle,
            prior,
            related,
            based_on,
            by,
            how,
            scope,
        } => run_remember(
            cli,
            &RememberArgs {
                slug,
                tldr,
                genre: *genre,
                topic,
                handle,
                prior: *prior,
                related,
                based_on,
                by: by.as_deref(),
                how: how.as_deref(),
                scope,
            },
        ),
        Command::Validate {
            slug,
            by,
            how,
            not_ok,
        } => run_validate(cli, slug, by, how, !*not_ok, None),
        Command::Refute {
            slug,
            by,
            how,
            instead,
        } => run_validate(cli, slug, by, how, false, instead.as_deref()),
    }
}

/// `remember`'s flags, grouped so the handler takes one argument rather than
/// nine positional ones a caller could transpose.
struct RememberArgs<'a> {
    slug: &'a str,
    tldr: &'a str,
    genre: Genre,
    topic: &'a [String],
    handle: &'a [String],
    prior: f64,
    related: &'a [String],
    based_on: &'a [String],
    by: Option<&'a str>,
    how: Option<&'a str>,
    scope: &'a str,
}

fn run_roots(cli: &Cli) -> Result<ExitCode> {
    // Reading the directories is the point: a row says how much each root held,
    // which is what turns "no hits" into either a genuine miss or a coverage
    // problem.
    let requested = roots(cli)?;
    let corpus = load_roots(requested.clone())?;
    let rows = report::root_rows(&requested, &corpus);

    // Collected before the payload takes ownership of the rows.
    let lines: Vec<String> = rows.iter().map(report::Columns::line).collect();
    emit(cli, &report::RootsOutput { roots: rows }, lines)
}

fn run_search(
    cli: &Cli,
    query: &str,
    limit: usize,
    topics: &[String],
    genres: &[Genre],
    all: bool,
) -> Result<ExitCode> {
    if query.trim().is_empty() {
        eprintln!("memories: search needs a query");
        return Ok(ExitCode::from(USAGE_EXIT_CODE));
    }

    let started = Instant::now();
    let requested = roots(cli)?;
    let corpus = load_roots(requested.clone())?;
    report_unparsed(&corpus);

    let hits = search::search(
        &corpus,
        &Query {
            text: query,
            limit,
            topics,
            genres,
            include_refuted: all,
        },
        Utc::now(),
    )?;

    let mut rendered = Vec::with_capacity(hits.len());
    for ranked in &hits {
        rendered.push(report::hit(
            &corpus.memories[ranked.memory],
            Some(Scores {
                bm25: ranked.bm25,
                score: ranked.score,
            }),
        )?);
    }

    let output = report::SearchOutput {
        query: query.to_owned(),
        roots: report::root_rows(&requested, &corpus),
        scanned: corpus.scanned(),
        elapsed_ms: started.elapsed().as_millis(),
        hits: rendered,
    };

    if cli.json {
        print_json(&output)?;
    } else {
        for hit in &output.hits {
            println!(
                "{score:>8.3}  {slug}{flags}  {tldr}",
                score = hit.score.unwrap_or_default(),
                slug = hit.slug,
                flags = flags(hit),
                tldr = hit.tldr,
            );
        }
        println!(
            "{hits} hits from {scanned} memories in {elapsed}ms",
            hits = output.hits.len(),
            scanned = output.scanned,
            elapsed = output.elapsed_ms,
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn run_show(cli: &Cli, slug: &str) -> Result<ExitCode> {
    let corpus = load(cli)?;
    report_unparsed(&corpus);

    let Some(memory) = corpus.by_slug(slug) else {
        eprintln!("memories: no memory matches the slug {slug}");
        return Ok(ExitCode::FAILURE);
    };
    // A slug is unique inside a directory but not across roots. The nearest
    // root wins, and the ones it shadows are named so the choice is visible.
    for shadowed in corpus.all_by_slug(slug).skip(1) {
        eprintln!(
            "memories: {slug} also exists at {path}, shadowed by {chosen}",
            path = shadowed.path.display(),
            chosen = memory.path.display(),
        );
    }

    let hit = report::hit(memory, None)?;
    if cli.json {
        print_json(&hit)?;
    } else {
        println!(
            "{slug}{flags}  {tldr}",
            flags = flags(&hit),
            tldr = hit.tldr
        );
        println!("{path}", path = hit.path);
        if let Some(reason) = &hit.stale_reason {
            println!("stale: {reason}");
        }
        for entry in &hit.validated {
            println!(
                "validated {at} by {by} ok={ok}: {how}",
                at = entry.at,
                by = entry.by,
                ok = entry.ok,
                how = entry.how,
            );
        }
        print!("{body}", body = hit.body);
    }
    Ok(ExitCode::SUCCESS)
}

fn run_stale(cli: &Cli) -> Result<ExitCode> {
    let corpus = load(cli)?;
    report_unparsed(&corpus);
    emit_rows(cli, report::stale_rows(&corpus)?)
}

fn run_refuted(cli: &Cli) -> Result<ExitCode> {
    let corpus = load(cli)?;
    report_unparsed(&corpus);
    emit_rows(cli, report::refuted_rows(&corpus))
}

fn run_unchecked(cli: &Cli, days: f64) -> Result<ExitCode> {
    let corpus = load(cli)?;
    report_unparsed(&corpus);
    emit_rows(cli, report::unchecked_rows(&corpus, Utc::now(), days))
}

fn run_lint(cli: &Cli, fix: bool) -> Result<ExitCode> {
    let corpus = load(cli)?;

    if fix {
        for memory in &corpus.memories {
            for change in write::fix(memory)? {
                println!("fixed {path}: {change}", path = memory.path.display());
            }
        }
        // Re-read, so the reported diagnostics describe the files as they are
        // now rather than as they were before the fixes.
        let fixed = load(cli)?;
        return emit_lint(cli, &fixed);
    }

    emit_lint(cli, &corpus)
}

fn emit_lint(cli: &Cli, corpus: &Corpus) -> Result<ExitCode> {
    let diagnostics = lint::lint(corpus, Utc::now())?;
    let errors = diagnostics.len();
    let output = report::LintOutput {
        diagnostics,
        errors,
        checked: corpus.scanned(),
    };

    if cli.json {
        print_json(&output)?;
    } else {
        for diagnostic in &output.diagnostics {
            println!("{}", diagnostic.render());
        }
        println!(
            "Errors: {errors}  Checked: {checked}",
            errors = output.errors,
            checked = output.checked
        );
    }

    Ok(if output.errors > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn run_remember(cli: &Cli, args: &RememberArgs<'_>) -> Result<ExitCode> {
    if !model::is_kebab_case(args.slug) {
        eprintln!(
            "memories: {slug} is not a kebab-case slug",
            slug = args.slug
        );
        return Ok(ExitCode::from(USAGE_EXIT_CODE));
    }

    let scope = match model::Scope::parse(args.scope) {
        Ok(scope) => scope,
        Err(bad) => {
            eprintln!("memories: --scope {bad} is not `shared` or `user:<name>`");
            return Ok(ExitCode::from(USAGE_EXIT_CODE));
        }
    };

    // A `genre: memory` file with no validation is genuinely incomplete, which is
    // why `memory-unchecked` fires on one. You write a memory at the moment you
    // learn something, and that is exactly the moment you still have the command
    // that proved it, so the proof is recorded at birth rather than in a second
    // command: otherwise the honest path costs two steps and the lazy one costs
    // one. A reference page is exempt from the rule, so it is exempt here too.
    let first_validation = match (args.by, args.how) {
        (Some(by), Some(how)) => Some(write::validation(by, how, true, Utc::now())),
        (None, None) if args.genre == Genre::Memory => {
            eprintln!(
                "memories: --by and --how are required for --genre memory. A memory without a \
                 re-runnable proof is a date: `--how` is what lets anyone, later, check whether \
                 it still holds. A `living` or `historical` page needs neither."
            );
            return Ok(ExitCode::from(USAGE_EXIT_CODE));
        }
        (None, None) => None,
        (by, _) => {
            // Half a validation is not a validation: an entry with no `how` cannot
            // be re-checked, and one with no `by` has nobody to ask.
            let missing = if by.is_none() { "--by" } else { "--how" };
            eprintln!("memories: {missing} is missing; a validation entry needs both");
            return Ok(ExitCode::from(USAGE_EXIT_CODE));
        }
    };

    let mut body = String::new();
    std::io::stdin()
        .read_to_string(&mut body)
        .context("reading the memory body from stdin")?;

    let roots = roots(cli)?;
    let Some(root) = roots.first() else {
        eprintln!("memories: no root to write to; pass --dir");
        return Ok(ExitCode::from(USAGE_EXIT_CODE));
    };

    let path = write::remember(
        root,
        &RememberSpec {
            slug: args.slug,
            tldr: args.tldr,
            genre: args.genre,
            topic: args.topic,
            handle: args.handle,
            prior: args.prior,
            related: args.related,
            based_on: args.based_on,
            scope,
            first_validation: first_validation.as_ref(),
            body: &body,
        },
    )?;
    println!("{path}", path = path.display());
    Ok(ExitCode::SUCCESS)
}

fn run_validate(
    cli: &Cli,
    slug: &str,
    by: &str,
    how: &str,
    ok: bool,
    instead: Option<&str>,
) -> Result<ExitCode> {
    let corpus = load(cli)?;
    let Some(memory) = corpus.by_slug(slug) else {
        eprintln!("memories: no memory matches the slug {slug}");
        return Ok(ExitCode::FAILURE);
    };

    // `--instead` resolves before anything is written, so a typo cannot leave
    // the refutation recorded and the successor unlinked.
    let successor = match instead.map(|other| (other, corpus.by_slug(other))) {
        None => None,
        Some((_, Some(resolved))) => Some(resolved),
        Some((other, None)) => {
            eprintln!("memories: --instead names {other}, which is not a memory");
            return Ok(ExitCode::FAILURE);
        }
    };

    let entry = write::validation(by, how, ok, Utc::now());
    let mut notes = write::append_validation(memory, &entry)?;
    if let Some(successor) = successor {
        notes.extend(write::add_supersedes(successor, slug)?);
    }

    for note in &notes {
        println!("{path}: {note}", path = memory.path.display());
    }
    Ok(ExitCode::SUCCESS)
}

fn emit_rows(cli: &Cli, rows: Vec<report::Row>) -> Result<ExitCode> {
    let mut lines: Vec<String> = rows.iter().map(report::Columns::line).collect();
    lines.push(format!("{count} rows", count = rows.len()));
    emit(cli, &report::RowsOutput { rows }, lines)
}

/// Print `payload` as JSON, or `lines` for a terminal.
///
/// One function rather than the same `if cli.json { … } else { for … } }` branch
/// in every handler: the two outputs are one decision, and writing it twice is
/// how a subcommand ends up printing human text into a JSON pipe.
fn emit<T: serde::Serialize>(
    cli: &Cli,
    payload: &T,
    lines: impl IntoIterator<Item = String>,
) -> Result<ExitCode> {
    if cli.json {
        print_json(payload)?;
    } else {
        for line in lines {
            println!("{line}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn load(cli: &Cli) -> Result<Corpus> {
    load_roots(roots(cli)?)
}

fn load_roots(requested: Vec<discover::Root>) -> Result<Corpus> {
    let looked_in: Vec<String> = requested
        .iter()
        .map(|root| root.memories_dir.display().to_string())
        .collect();
    let corpus = discover::load(requested)?;
    // An empty result because nothing exists reads exactly like an empty result
    // because nothing matched. Say which directories were looked at instead.
    if corpus.scans.is_empty() {
        eprintln!(
            "memories: no `.memories` directory in any search root ({roots})",
            roots = looked_in.join(", ")
        );
    }
    Ok(corpus)
}

fn roots(cli: &Cli) -> Result<Vec<discover::Root>> {
    let cwd = std::env::current_dir().context("reading the current directory")?;
    Ok(discover::resolve_roots(&cli.dirs, &cwd)?)
}

/// A file in a `.memories` directory that is not a memory is never dropped in
/// silence. `search --json`'s shape has no place for a diagnostic, so these go
/// to stderr, where they cannot corrupt the JSON a caller is parsing.
fn report_unparsed(corpus: &Corpus) {
    for failure in &corpus.failures {
        let location = failure.line.map_or_else(
            || failure.path.display().to_string(),
            |line| format!("{path}:{line}", path = failure.path.display()),
        );
        eprintln!(
            "memories: {location}: {rule}: {message}",
            rule = failure.rule,
            message = failure.message
        );
    }
}

fn flags(hit: &report::Hit) -> String {
    let mut flags = String::new();
    if hit.scope != "shared" {
        flags.push_str(" [");
        flags.push_str(&hit.scope);
        flags.push(']');
    }
    if hit.stale {
        flags.push_str(" [stale]");
    }
    if hit.refuted {
        flags.push_str(" [refuted]");
    }
    flags
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    let json = serde_json::to_string_pretty(value).context("serializing JSON output")?;
    println!("{json}");
    Ok(())
}
