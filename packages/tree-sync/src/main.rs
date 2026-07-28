//! `tree-sync`: push a source tree to a remote host or another checkout.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use clap::Parser;
use color_eyre::Result;
use color_eyre::eyre::{WrapErr as _, eyre};
use tree_sync::filter::Filter;
use tree_sync::target::Target;
use tree_sync::transfer;
use tree_sync::tree;

/// Sync a source tree using git's view of it.
///
/// The file set is what git reports: tracked files plus untracked files that
/// `.gitignore` does not cover. Build output is therefore excluded because the
/// repo says so. Extra `--exclude` patterns are anchored at the source root, so
/// `--exclude 'result*'` cannot reach `crates/codec/src/impls/result.rs` the way
/// the same rsync flag does.
#[derive(Debug, Parser)]
#[command(name = "tree-sync", version, about, long_about = None)]
struct Args {
    /// Directory to sync from.
    source: PathBuf,

    /// Where to sync to: `host:/path` over ssh, or a local directory.
    destination: String,

    /// Exclude a path, anchored at the source root. Repeatable. gitignore
    /// syntax, except that a pattern with no leading `/` or `**/` is pinned to
    /// the root rather than floating to every depth.
    #[arg(short = 'e', long = "exclude", value_name = "PATTERN")]
    exclude: Vec<String>,

    /// Exclude a path at every depth, which is rsync's default reading of a
    /// bare pattern. Repeatable. Spelled out so the wider match is a choice.
    #[arg(long = "exclude-any-depth", value_name = "PATTERN")]
    exclude_any_depth: Vec<String>,

    // --all and --delete are one decision: how much of what the destination
    // reports about itself the sync honours. --all discards its claim that a
    // file is already current; --delete discards its right to keep a file the
    // source does not have. Flattened behind a plain comment, because clap
    // renders a doc comment on a flattened field as a --help group heading.
    #[command(flatten)]
    reconcile: ReconcileArgs,

    /// Report what would move, and move nothing.
    #[arg(short = 'n', long)]
    dry_run: bool,

    /// Extra `-o` option for ssh. Repeatable.
    #[arg(long = "ssh-option", value_name = "OPTION")]
    ssh_option: Vec<String>,

    /// Print only errors.
    #[arg(short, long)]
    quiet: bool,
}

/// How far the destination's own account of itself is allowed to limit the sync.
#[derive(Debug, clap::Args)]
struct ReconcileArgs {
    /// Send every file, skipping the size and mtime up-to-date check.
    #[arg(long)]
    all: bool,

    /// Remove destination files the source no longer has.
    #[arg(long)]
    delete: bool,
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let args = Args::parse();
    run(&args)
}

fn run(args: &Args) -> Result<()> {
    let source = args
        .source
        .canonicalize()
        .wrap_err_with(|| format!("no such source directory: {}", args.source.display()))?;
    if !source.is_dir() {
        return Err(eyre!("{} is not a directory", source.display()));
    }
    let target = Target::parse(&args.destination)?;

    let listing = tree::list(&source)?;
    let scanned = listing.entries.len();
    let scanned_bytes = listing.bytes();

    let mut excludes = Filter::new(&source, &args.exclude, &args.exclude_any_depth)?;
    let selected: Vec<tree::Entry> = listing
        .entries
        .into_iter()
        .filter(|entry| !excludes.excludes(&entry.relative))
        .collect();

    let say = |line: &str| {
        if !args.quiet {
            println!("{line}");
        }
    };

    say(&format!(
        "tree-sync  {} -> {}{}",
        source.display(),
        target.describe(),
        if args.dry_run { "  (dry run)" } else { "" }
    ));
    say(&format!("  source    {}", listing.origin.describe()));
    say(&format!(
        "            {scanned} files, {}",
        human_bytes(scanned_bytes)
    ));
    for rule in excludes.rules() {
        let note = if rule.hits == 0 {
            "  (matched nothing)"
        } else {
            ""
        };
        say(&format!(
            "  exclude   {:?} matched as {:?}: {} paths{note}",
            rule.given, rule.effective, rule.hits
        ));
    }
    if !excludes.is_empty() {
        say(&format!(
            "  selected  {} files, {}",
            selected.len(),
            human_bytes(selected.iter().map(|entry| entry.size).sum())
        ));
    }

    if selected.is_empty() && args.reconcile.delete {
        return Err(eyre!(
            "the source tree selected no files, so --delete would empty {}; \
             check the source path and the exclude patterns",
            target.describe()
        ));
    }

    let remote = match &target {
        Target::Local { .. } => None,
        Target::Remote { host, .. } => {
            Some(transfer::Remote::new(host.clone(), args.ssh_option.clone()))
        }
    };
    let dest = target.path();

    let moved = match remote.as_ref() {
        Some(remote) => remote.push(&source, &selected, dest, args.reconcile.all, args.dry_run)?,
        None => transfer::push_local(&source, &selected, dest, args.reconcile.all, args.dry_run)?,
    };

    if moved.manifest_unavailable {
        say(
            "  note      the destination could not list itself (no GNU find?), \
             so every file was sent",
        );
    }
    say(&format!(
        "  sent      {} files, {} ({} already current)",
        moved.files,
        human_bytes(moved.bytes),
        moved.unchanged
    ));

    if args.reconcile.delete {
        delete_removed(remote.as_ref(), dest, &target, &selected, args.dry_run, say)?;
    }

    Ok(())
}

/// Remove the paths the destination holds that the source no longer selects.
///
/// Split out of `run` so the sync's two halves, sending and removing, each read
/// on their own. A destination that cannot list itself is an error rather than
/// an empty deletion set: guessing here removes the operator's files.
fn delete_removed(
    remote: Option<&transfer::Remote>,
    dest: &Path,
    target: &Target,
    selected: &[tree::Entry],
    dry_run: bool,
    say: impl Fn(&str),
) -> Result<()> {
    let keep: HashSet<PathBuf> = selected
        .iter()
        .map(|entry| entry.relative.clone())
        .collect();
    let present = transfer::manifest_for(remote, dest)?.ok_or_else(|| {
        eyre!(
            "--delete needs the destination to list itself, and {} could not; \
             refusing to guess at what to remove",
            target.describe()
        )
    })?;
    let doomed = transfer::plan_deletions(&present, &keep)?;
    match remote {
        Some(remote) => remote.delete(dest, &doomed, dry_run)?,
        None => transfer::delete_local(dest, &doomed, dry_run)?,
    }
    say(&format!("  deleted   {} paths", doomed.len()));
    for path in doomed.iter().take(DELETION_SAMPLE) {
        say(&format!("            {}", path.display()));
    }
    if doomed.len() > DELETION_SAMPLE {
        say(&format!(
            "            ... and {} more",
            doomed.len() - DELETION_SAMPLE
        ));
    }
    Ok(())
}

/// How many deleted paths to name before summarising the rest.
const DELETION_SAMPLE: usize = 10;

/// Render a byte count the way an operator reads one. Integer arithmetic
/// throughout, so no size can round to a misleading zero.
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut whole = bytes;
    let mut tenths = 0;
    let mut unit = 0;
    while whole >= 1024 && unit + 1 < UNITS.len() {
        tenths = (whole % 1024) * 10 / 1024;
        whole /= 1024;
        unit += 1;
    }
    let name = UNITS.get(unit).copied().unwrap_or("B");
    if unit == 0 {
        format!("{whole} {name}")
    } else {
        format!("{whole}.{tenths} {name}")
    }
}

#[cfg(test)]
mod tests {
    use super::human_bytes;

    #[test]
    fn byte_counts_read_the_way_an_operator_expects() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(64 * 1024 * 1024), "64.0 MiB");
    }
}
