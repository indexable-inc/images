//! Command line surface over the view filter.
//!
//! Four operations, covering both directions a view moves in.
//!
//! `import` injects a standalone repository's history under a prefix as its own
//! root lineage. It deliberately takes no monorepo revision to sit on top of: a
//! root commit of the source stays a root commit here, so such an option could
//! only decide whose tree the root starts from, and pointing it at a monorepo
//! tip would produce a parentless commit carrying the entire monorepo. The
//! injected lineage joins the monorepo by an ordinary merge instead, which is a
//! single `git merge` because the two sides touch disjoint paths.
//!
//! `derive` computes the view of a monorepo revision and points a ref at it.
//! That ref's history is byte for byte the standalone repository's, so
//! publishing it is an ordinary push and following it is an ordinary fetch.
//!
//! `unfilter` lifts commits made on the derived side back into the monorepo. It
//! derives `--onto` first, which is what teaches it where each view commit
//! already lives; a lift with no such knowledge has no ancestry to attach to.
//! The result sits on the injected lineage rather than on the monorepo tip, and
//! merging it forward is a second operation. That split is the point:
//! reverse-applying a *rebased* view onto the monorepo is exactly how a derived
//! view turns into a non-fast-forward rewrite of the monorepo.
//!
//! `verify` derives every commit reachable from a monorepo revision and reports
//! how many of the standalone repository's hashes came back.

use std::collections::HashMap;
use std::path::PathBuf;

use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;
use gix::ObjectId;
use jj_views::Cache;
use jj_views::Elide;
use jj_views::Filter;
use jj_views::Semantics;
use jj_views::verify::ancestry;
use jj_views::verify::derived_set;

type Failure = Box<dyn std::error::Error + Send + Sync>;

/// Derive a child repository from a monorepo path, and lift its commits back.
#[derive(Parser)]
#[command(name = "jj-views", version)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Import(Import),
    Derive(Derive),
    Unfilter(Unfilter),
    Verify(Verify),
}

/// Which commits a view leaves out.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum ElideRule {
    /// Keep every commit of the parent history.
    Nothing,
    /// Drop a commit that changed nothing under the prefix, but only when it
    /// changed something outside it. josh's rule, and the only one here that
    /// preserves hashes.
    Unchanged,
    /// Also drop a commit that was already empty before filtering. Reads as the
    /// obvious simplification of the rule above and silently breaks hash
    /// identity; it exists so a test can demonstrate that.
    UnchangedIncludingAlreadyEmpty,
}

impl From<ElideRule> for Elide {
    fn from(rule: ElideRule) -> Self {
        match rule {
            ElideRule::Nothing => Self::Nothing,
            ElideRule::Unchanged => Self::Unchanged,
            ElideRule::UnchangedIncludingAlreadyEmpty => Self::UnchangedIncludingAlreadyEmpty,
        }
    }
}

/// Which recorded rule set to apply.
///
/// Defaults to the newest, unlike the library, whose default has to stay V1 so
/// existing callers keep their output. A view here is always freshly built, and
/// V1's lifting rule leaves a merge commit in the view once per round trip.
#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum Rules {
    /// The initial rules. Lifting takes whichever counterpart the cache learned
    /// first, which depends on the order things were derived in.
    V1,
    /// V1 with lifting specified: a parent that `--onto` derives to lands on
    /// `--onto`, so a lift is a fast-forward and a round trip adds no commit.
    #[default]
    V2,
}

impl From<Rules> for Semantics {
    fn from(rules: Rules) -> Self {
        match rules {
            Rules::V1 => Self::V1,
            Rules::V2 => Self::V2,
        }
    }
}

/// What every operation needs: a repository and a filter.
#[derive(clap::Args)]
struct Common {
    /// Repository to work in. Defaults to the one containing the working
    /// directory.
    #[arg(long, short = 'R', value_name = "DIR", global = true)]
    repo: Option<PathBuf>,
    /// Path prefix the view is of.
    #[arg(long, value_name = "PREFIX")]
    path: String,
    /// Which commits to leave out of the view.
    #[arg(long, value_enum, default_value_t = ElideRule::Unchanged)]
    elide: ElideRule,
    /// Which recorded rule set to apply.
    #[arg(long, value_enum, default_value_t = Rules::default())]
    semantics: Rules,
}

impl Common {
    fn open(&self) -> Result<(gix::Repository, Filter), Failure> {
        let repo = match &self.repo {
            Some(dir) => gix::open(dir)?,
            None => gix::discover(".")?,
        };
        // Writing a ref writes a reflog entry, and a reflog entry needs a
        // committer. A bare store -- which is exactly what `-R .jj/repo/store/git`
        // points at -- often has no identity configured, and the failure is a
        // bare `MissingCommitter` raised after the commits have already been
        // written, so the work looks lost when it is not. Supply one only when
        // the repository has none, so a configured identity still wins.
        let repo = if repo.committer().is_some() {
            repo
        } else {
            gix::open_opts(
                repo.path(),
                gix::open::Options::default()
                    .config_overrides([
                        "committer.name=jj-views",
                        "committer.email=jj-views@invalid",
                    ])
                    .open_path_as_is(true),
            )?
        };
        let filter = Filter::prefix(&self.path)?
            .semantics(self.semantics.into())
            .elide(self.elide.into());
        Ok((repo, filter))
    }
}

/// Inject a standalone repository's history under the prefix.
#[derive(clap::Args)]
struct Import {
    #[command(flatten)]
    common: Common,
    /// Tip of the history to inject. Its objects have to be in the repository
    /// already, which a plain `git fetch` of the source puts there.
    #[arg(long, value_name = "REV")]
    rev: String,
    /// Ref to point at the injected tip.
    #[arg(long, value_name = "REF")]
    write_ref: Option<String>,
}

/// Compute the view of a monorepo revision.
#[derive(clap::Args)]
struct Derive {
    #[command(flatten)]
    common: Common,
    /// Monorepo revision to derive.
    #[arg(long, value_name = "REV")]
    rev: String,
    /// Ref to point at the derived tip.
    #[arg(long, value_name = "REF")]
    write_ref: Option<String>,
}

/// Lift commits made on the derived side back into the monorepo.
#[derive(clap::Args)]
struct Unfilter {
    #[command(flatten)]
    common: Common,
    /// Tip of the view history to lift. Everything in its ancestry that the
    /// monorepo does not already account for is lifted, parents first.
    #[arg(long, value_name = "REV")]
    rev: String,
    /// Monorepo revision the view was made on top of. Deriving it is what says
    /// where the lifted commits' parents already live; a parent still unknown
    /// after that lands its child here instead.
    #[arg(long, value_name = "REV")]
    onto: String,
    /// Ref to point at the lifted tip.
    #[arg(long, value_name = "REF")]
    write_ref: Option<String>,
}

/// Check that deriving gives the standalone repository's hashes back.
#[derive(clap::Args)]
struct Verify {
    #[command(flatten)]
    common: Common,
    /// Monorepo revision to derive.
    #[arg(long, value_name = "REV")]
    rev: String,
    /// Tip of the standalone history every derived hash has to match.
    #[arg(long, value_name = "REV")]
    against: String,
}

fn main() -> Result<(), Failure> {
    match Args::parse().command {
        Command::Import(args) => import(&args),
        Command::Derive(args) => derive(&args),
        Command::Unfilter(args) => unfilter(&args),
        Command::Verify(args) => verify(&args),
    }
}

fn import(args: &Import) -> Result<(), Failure> {
    let (repo, filter) = args.common.open()?;
    let tip = rev_parse(&repo, &args.rev)?;
    let order = ancestry(&repo, &tip)?;
    let base = empty_base(&repo)?;

    let mut cache = Cache::new();
    let mut injected: HashMap<ObjectId, ObjectId> = HashMap::new();
    for source in &order {
        let raw = repo.find_object(*source)?.detach().data;
        let first = gix::objs::CommitRef::from_bytes(&raw, repo.object_hash())?
            .parents()
            .next();
        let onto = first
            .and_then(|parent| injected.get(&parent).copied())
            .unwrap_or(base);
        let id = jj_views::unfilter(&repo, source, &onto, &filter, &mut cache)?;
        injected.insert(*source, id);
    }

    let head = injected
        .get(&tip)
        .copied()
        .ok_or("the source tip was not injected")?;
    println!(
        "injected {} commits under {}",
        injected.len(),
        filter.path()
    );
    println!("{head}");
    maybe_write_ref(&repo, args.write_ref.as_deref(), head)
}

fn derive(args: &Derive) -> Result<(), Failure> {
    let (repo, filter) = args.common.open()?;
    let rev = rev_parse(&repo, &args.rev)?;
    let mut cache = Cache::new();
    let Some(head) = jj_views::derive(&repo, &rev, &filter, &mut cache)? else {
        return Err(format!(
            "nothing under {} anywhere in the ancestry of {}",
            filter.path(),
            args.rev
        )
        .into());
    };
    println!("{head}");
    maybe_write_ref(&repo, args.write_ref.as_deref(), head)
}

fn unfilter(args: &Unfilter) -> Result<(), Failure> {
    let (repo, filter) = args.common.open()?;
    let rev = rev_parse(&repo, &args.rev)?;
    let onto = rev_parse(&repo, &args.onto)?;

    let mut cache = Cache::new();
    let known = derived_set(&repo, &onto, &filter, &mut cache)?;

    let mut lifted: HashMap<ObjectId, ObjectId> = HashMap::new();
    for view in ancestry(&repo, &rev)? {
        if known.contains(&view) {
            continue;
        }
        let id = jj_views::unfilter(&repo, &view, &onto, &filter, &mut cache)?;
        lifted.insert(view, id);
    }

    let head = lifted.get(&rev).copied().ok_or_else(|| {
        format!(
            "{} is already part of the monorepo history at {}, so there is nothing to lift",
            args.rev, args.onto
        )
    })?;
    println!("lifted {} commits", lifted.len());
    println!("{head}");
    maybe_write_ref(&repo, args.write_ref.as_deref(), head)
}

fn verify(args: &Verify) -> Result<(), Failure> {
    let (repo, filter) = args.common.open()?;
    let rev = rev_parse(&repo, &args.rev)?;
    let against = rev_parse(&repo, &args.against)?;

    let mut cache = Cache::new();
    let report = jj_views::verify::verify(&repo, &rev, &against, &filter, &mut cache)?;

    // The tip is reported on its own even though it is one of the hashes
    // counted below, because it is the one that makes the others follow: a
    // commit hash covers its parents transitively, so a matching tip means the
    // whole reachable history matched byte for byte.
    match report.tip {
        Some(id) if report.tip_matches() => println!("tip {id} matches"),
        Some(id) => println!("tip {id} does not match {against}"),
        None => println!("tip has no view under {}", filter.path()),
    }
    println!(
        "{} of {} commits identical",
        report.expected - report.missing.len(),
        report.expected
    );
    report_sample("missing from the view", &report.missing);
    report_sample("in the view but not upstream", &report.extra);

    if report.identical() {
        Ok(())
    } else {
        Err("the derived history is not the standalone history".into())
    }
}

fn report_sample(label: &str, ids: &[ObjectId]) {
    if ids.is_empty() {
        return;
    }
    println!("{} {label}:", ids.len());
    for id in ids.iter().take(10) {
        println!("  {id}");
    }
}

/// A commit with an empty tree, for a lifted root to take its tree base from.
///
/// Nothing points at it and nothing will: [`jj_views::unfilter`] reads a base's
/// tree, and for a root it does not make that base a parent. An empty tree
/// keeps the injected lineage to exactly the prefix. The empty tree object is
/// written too, since a commit naming an object git does not have fails
/// `git fsck`.
fn empty_base(repo: &gix::Repository) -> Result<ObjectId, Failure> {
    let tree = repo.write_object(gix::objs::Tree::default())?.detach();
    let raw = format!(
        "tree {tree}\nauthor views <views@invalid> 0 +0000\ncommitter views <views@invalid> 0 \
         +0000\n\nempty base\n"
    );
    gix::objs::Write::write_buf(&repo.objects, gix::objs::Kind::Commit, raw.as_bytes())
}

/// Resolves a ref name or a full object id to the commit it names.
///
/// Deliberately not gix's `rev_parse_single`: that wants the `revision` feature
/// on the workspace's gix, and widening a dependency every other crate here
/// builds, so this one can accept `HEAD~2`, is a poor trade. Callers are
/// scripts, which have a ref or an id in hand either way.
fn rev_parse(repo: &gix::Repository, rev: &str) -> Result<ObjectId, Failure> {
    let id = match repo.find_reference(rev) {
        Ok(mut reference) => reference.peel_to_id()?.detach(),
        Err(err) => ObjectId::from_hex(rev.as_bytes()).map_err(|_| err)?,
    };
    Ok(repo
        .find_object(id)?
        .peel_to_kind(gix::objs::Kind::Commit)?
        .id)
}

fn maybe_write_ref(
    repo: &gix::Repository,
    name: Option<&str>,
    id: ObjectId,
) -> Result<(), Failure> {
    let Some(name) = name else {
        return Ok(());
    };
    repo.reference(
        name,
        id,
        gix::refs::transaction::PreviousValue::Any,
        "jj-views",
    )?;
    println!("wrote {name}");
    Ok(())
}
