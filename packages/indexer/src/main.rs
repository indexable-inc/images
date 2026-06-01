//! `indexer`: sync every configured corpus source into Mixedbread (semantic
//! search) and a self-hosted S3/R2 parquet archive (polars/duckdb-queryable).
//!
//! Each source is an adapter implementing [`source_meta::SourceAdapter`]; the
//! indexer fans every selected source out to both sinks, reusing the
//! `search-core` Mixedbread reconcile (skip-if-unchanged) and the generic
//! [`sink_parquet`] sink. Pass `--mixedbread-store` and/or `--bucket` to enable a
//! sink, and one or more source flags to choose what to ingest.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use clap::Parser;
use search_core::MixedbreadStore;
use sink_mixedbread::sync_documents;

/// Manifest limits for code repos, matching `search-core`'s defaults.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Cap on new files uploaded per code sync (a runaway guard).
const MAX_FILES: usize = 10_000;
use source_meta::SourceAdapter;

/// How long to wait for Mixedbread to finish embedding new documents.
const INDEX_TIMEOUT: Duration = Duration::from_mins(2);

/// Sync corpus sources to Mixedbread and/or an S3/R2 parquet archive.
#[derive(Debug, Parser)]
#[command(name = "indexer", about, version)]
struct Cli {
    /// Mixedbread store name; enables the Mixedbread (semantic) sink.
    #[arg(long, env = "MXBAI_STORE")]
    mixedbread_store: Option<String>,

    /// Mixedbread API base URL.
    #[arg(long = "base-url", env = "MXBAI_BASE_URL")]
    base_url: Option<String>,

    /// Bucket for the parquet archive; enables the S3/R2 sink. Credentials come
    /// from `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`.
    #[arg(long, env = "INDEXER_BUCKET")]
    bucket: Option<String>,

    /// S3 endpoint URL; empty means AWS S3, for R2 pass the account endpoint.
    #[arg(long, env = "INDEXER_S3_ENDPOINT")]
    endpoint: Option<String>,

    /// S3 region (`auto` for R2).
    #[arg(long, env = "INDEXER_S3_REGION", default_value = "auto")]
    region: String,

    /// Key prefix under the bucket.
    #[arg(long, env = "INDEXER_PREFIX", default_value = "corpus")]
    prefix: String,

    /// Index local agent/shell history (claude, codex, atuin) at their default
    /// paths, in addition to any explicit `--*` overrides below.
    #[arg(long)]
    local: bool,

    /// Claude Code transcript directory (default with `--local`: `~/.claude/projects`).
    #[arg(long)]
    claude_dir: Option<PathBuf>,

    /// Codex history file (default with `--local`: `~/.codex/history.jsonl`).
    #[arg(long)]
    codex_file: Option<PathBuf>,

    /// atuin history db (default with `--local`: `~/.local/share/atuin/history.db`).
    #[arg(long)]
    atuin_db: Option<PathBuf>,

    /// Slack export directory.
    #[arg(long)]
    slack_export: Option<PathBuf>,

    /// Linear export directory.
    #[arg(long)]
    linear_export: Option<PathBuf>,

    /// Git repository to index commit history from (repeatable).
    #[arg(long = "git-repo")]
    git_repos: Vec<PathBuf>,

    /// Code checkout to index (content-addressed, like a bare `search`).
    /// Mixedbread only (code lives in git, not the parquet archive); repeatable.
    #[arg(long = "code-repo")]
    code_repos: Vec<PathBuf>,

    /// Index one user's local history (claude, codex, atuin) by `NAME:HOME`,
    /// repeatable. One process indexes many users — the fleet runs this as root
    /// over every account — tagging each user's records with `NAME`. Symlinked
    /// history paths are skipped so a privileged run cannot be a confused deputy.
    #[arg(long = "user", value_name = "NAME:HOME")]
    users: Vec<String>,

    /// Host name to tag `--user` records with. Defaults to the system hostname;
    /// the fleet module passes the NixOS `networking.hostName`.
    #[arg(long)]
    host: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let store = match &cli.mixedbread_store {
        Some(_) => {
            let base_url =
                cli.base_url.clone().unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
            Some(MixedbreadStore::from_login(base_url).await.context("connecting to Mixedbread")?)
        }
        None => None,
    };
    let parquet = cli.bucket.as_ref().map(|bucket| sink_parquet::Config {
        bucket: bucket.clone(),
        endpoint: cli.endpoint.clone(),
        region: cli.region.clone(),
        prefix: cli.prefix.clone(),
    });
    if store.is_none() && parquet.is_none() {
        anyhow::bail!("nothing to do: pass --mixedbread-store and/or --bucket");
    }
    if !any_source_selected(&cli) {
        anyhow::bail!(
            "no sources selected: pass --local, --user NAME:HOME, --claude-dir/--codex-file/--atuin-db/--slack-export/--linear-export/--git-repo, or --code-repo"
        );
    }
    let mixedbread = store.as_ref().zip(cli.mixedbread_store.as_deref());

    let (indexed, failures) = run_sources(&cli, mixedbread, parquet.as_ref()).await;

    if failures > 0 {
        anyhow::bail!("{failures} of {} source(s) failed; {indexed} succeeded", indexed + failures);
    }
    Ok(())
}

/// Whether any source flag was given (a config check, independent of how many
/// records each source ends up producing).
const fn any_source_selected(cli: &Cli) -> bool {
    cli.local
        || cli.claude_dir.is_some()
        || cli.codex_file.is_some()
        || cli.atuin_db.is_some()
        || cli.slack_export.is_some()
        || cli.linear_export.is_some()
        || !cli.git_repos.is_empty()
        || !cli.code_repos.is_empty()
        || !cli.users.is_empty()
}

/// Resolve the selected sources and run each one independently (a failure never
/// aborts the others), returning `(indexed, failed)` counts.
async fn run_sources(
    cli: &Cli,
    mixedbread: Option<(&MixedbreadStore, &str)>,
    parquet: Option<&sink_parquet::Config>,
) -> (usize, usize) {
    let home = dirs::home_dir();
    let default = |suffix: &str| home.as_ref().map(|h| h.join(suffix));
    let claude = cli.claude_dir.clone().or_else(|| cli.local.then(|| default(".claude/projects")).flatten());
    let codex = cli.codex_file.clone().or_else(|| cli.local.then(|| default(".codex/history.jsonl")).flatten());
    let atuin = cli.atuin_db.clone().or_else(|| cli.local.then(|| default(".local/share/atuin/history.db")).flatten());

    let mut indexed = 0_usize;
    let mut failures = 0_usize;
    if let Some(dir) = claude {
        let result = async {
            let adapter = source_claude::ClaudeHistoryExport::open(&dir)
                .with_context(|| format!("parsing Claude transcripts at {}", dir.display()))?;
            run_source("claude", &adapter, mixedbread, parquet).await
        }
        .await;
        record("claude", result, &mut indexed, &mut failures);
    }
    if let Some(file) = codex {
        let result = async {
            let adapter = source_codex::CodexHistory::open(&file)
                .with_context(|| format!("parsing Codex history at {}", file.display()))?;
            run_source("codex", &adapter, mixedbread, parquet).await
        }
        .await;
        record("codex", result, &mut indexed, &mut failures);
    }
    if let Some(db) = atuin {
        let result = async {
            let adapter = source_atuin::AtuinHistory::open(&db)
                .with_context(|| format!("reading atuin history at {}", db.display()))?;
            run_source("shell", &adapter, mixedbread, parquet).await
        }
        .await;
        record("shell", result, &mut indexed, &mut failures);
    }
    if let Some(dir) = &cli.slack_export {
        let result = async {
            let adapter = source_slack::SlackExport::open(dir)
                .with_context(|| format!("reading Slack export at {}", dir.display()))?;
            run_source("slack", &adapter, mixedbread, parquet).await
        }
        .await;
        record("slack", result, &mut indexed, &mut failures);
    }
    if let Some(dir) = &cli.linear_export {
        let result = async {
            let adapter = source_linear::LinearExport::open(dir)
                .with_context(|| format!("reading Linear export at {}", dir.display()))?;
            run_source("linear", &adapter, mixedbread, parquet).await
        }
        .await;
        record("linear", result, &mut indexed, &mut failures);
    }
    for repo in &cli.git_repos {
        let label = format!("git:{}", repo.display());
        let result = async {
            let adapter = source_git::GitLog::open(repo)
                .with_context(|| format!("reading git history at {}", repo.display()))?;
            run_source("git", &adapter, mixedbread, parquet).await
        }
        .await;
        record(&label, result, &mut indexed, &mut failures);
    }
    for repo_dir in &cli.code_repos {
        let label = format!("code:{}", repo_dir.display());
        let result = index_code(&label, repo_dir, mixedbread).await;
        record(&label, result, &mut indexed, &mut failures);
    }
    if !cli.users.is_empty() {
        run_users(cli, mixedbread, parquet, &mut indexed, &mut failures).await;
    }
    (indexed, failures)
}

/// Run the `--user NAME:HOME` multi-user phase, accumulating into the shared
/// counters. Split out of [`run_sources`] to keep each function focused.
async fn run_users(
    cli: &Cli,
    mixedbread: Option<(&MixedbreadStore, &str)>,
    parquet: Option<&sink_parquet::Config>,
    indexed: &mut usize,
    failures: &mut usize,
) {
    let host = match resolve_host(cli) {
        Ok(host) => host,
        Err(error) => {
            // Without a host tag every claude/codex record would be mislabeled,
            // so fail the whole multi-user phase rather than emit wrong metadata.
            // Count it as one phase failure (no per-user work ran).
            eprintln!("[users] failed to resolve host, skipping all --user sources: {error:#}");
            *failures += 1;
            return;
        }
    };
    for spec in &cli.users {
        match parse_user(spec) {
            Ok((name, home)) => {
                index_user(&name, &home, &host, mixedbread, parquet, indexed, failures).await;
            }
            Err(error) => {
                eprintln!("[users] bad --user spec: {error:#}");
                *failures += 1;
            }
        }
    }
}

/// Index one user's local agent and shell history (claude, codex, atuin),
/// reading under `home` and tagging records with `name` and `host`.
///
/// Designed for the privileged fleet run. Every history path is resolved with
/// [`safe_path_under`], which refuses to follow a symlink at any user-controlled
/// component, so a planted symlink cannot redirect a root read at another
/// account's files (the claude adapter additionally refuses symlinks inside its
/// tree). Absent sources are skipped; a parse failure in one source is recorded
/// but does not abort the others.
async fn index_user(
    name: &str,
    home: &Path,
    host: &str,
    mixedbread: Option<(&MixedbreadStore, &str)>,
    parquet: Option<&sink_parquet::Config>,
    indexed: &mut usize,
    failures: &mut usize,
) {
    if let Some(claude_dir) = safe_path_under(home, &[".claude", "projects"], true) {
        let label = format!("claude:{name}");
        let parquet = parquet.map(|config| user_parquet(config, name));
        let result = async {
            let adapter = source_claude::ClaudeHistoryExport::open_with(&claude_dir, host, name)
                .with_context(|| format!("parsing Claude transcripts for {name} at {}", claude_dir.display()))?;
            run_source(&label, &adapter, mixedbread, parquet.as_ref()).await
        }
        .await;
        record(&label, result, indexed, failures);
    }

    if let Some(codex_file) = safe_path_under(home, &[".codex", "history.jsonl"], false) {
        let label = format!("codex:{name}");
        let parquet = parquet.map(|config| user_parquet(config, name));
        let result = async {
            let adapter = source_codex::CodexHistory::open_with(&codex_file, host, name)
                .with_context(|| format!("parsing Codex history for {name} at {}", codex_file.display()))?;
            run_source(&label, &adapter, mixedbread, parquet.as_ref()).await
        }
        .await;
        record(&label, result, indexed, failures);
    }

    // atuin records its own `host`/`user` in each row, so it self-tags per user
    // regardless of who runs the process.
    if let Some(atuin_db) = safe_path_under(home, &[".local", "share", "atuin", "history.db"], false) {
        let label = format!("shell:{name}");
        let parquet = parquet.map(|config| user_parquet(config, name));
        let result = async {
            let adapter = source_atuin::AtuinHistory::open(&atuin_db)
                .with_context(|| format!("reading atuin history for {name} at {}", atuin_db.display()))?;
            run_source(&label, &adapter, mixedbread, parquet.as_ref()).await
        }
        .await;
        record(&label, result, indexed, failures);
    }
}

/// Parse a `NAME:HOME` user spec. The name is everything before the first colon;
/// both parts must be non-empty.
fn parse_user(spec: &str) -> anyhow::Result<(String, PathBuf)> {
    let (name, home) =
        spec.split_once(':').with_context(|| format!("--user must be NAME:HOME, got {spec:?}"))?;
    anyhow::ensure!(!name.is_empty(), "--user NAME must be non-empty in {spec:?}");
    anyhow::ensure!(!home.is_empty(), "--user HOME must be non-empty in {spec:?}");
    // NAME becomes a metadata tag and a `user=<name>` parquet partition segment,
    // so keep it to a safe charset (no `/` or `=` that could cross partitions).
    anyhow::ensure!(
        name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
        "--user NAME must be ascii alphanumeric plus `.`/`_`/`-`, got {name:?}"
    );
    Ok((name.to_owned(), PathBuf::from(home)))
}

/// A per-user parquet config: partition each user's rows under `user=<name>` so
/// concurrently indexed users never overwrite the one shared per-source file.
fn user_parquet(config: &sink_parquet::Config, name: &str) -> sink_parquet::Config {
    sink_parquet::Config {
        prefix: format!("{}/user={name}", config.prefix),
        ..config.clone()
    }
}

/// Resolve a user-controlled subpath under a trusted `home`, refusing to follow
/// a symlink at any component. Returns the joined path only if every component in
/// `rel` is a real (non-symlink) entry — intermediate ones directories, and the
/// final one matching `want_dir` (a directory) or `!want_dir` (a regular file).
///
/// `home` comes from the system user database and is trusted; everything under
/// it is attacker-controlled when this runs as root over other accounts. lstat'ing
/// every component (not just the leaf) is what stops a planted symlink — at the
/// root, an ancestor, or the leaf — from redirecting the read at another
/// account's files (the confused-deputy class; see ix `history-ship`).
///
/// A narrow TOCTOU remains: a component could be swapped for a symlink between
/// this check and the adapter's open. That residual race matches `history-ship`'s
/// posture and is tracked for a shared `openat2(RESOLVE_NO_SYMLINKS)` hardening
/// across both readers.
fn safe_path_under(home: &Path, rel: &[&str], want_dir: bool) -> Option<PathBuf> {
    let last = rel.len().checked_sub(1)?;
    let mut path = home.to_path_buf();
    for (index, component) in rel.iter().enumerate() {
        path.push(component);
        let meta = std::fs::symlink_metadata(&path).ok()?;
        if meta.file_type().is_symlink() {
            return None;
        }
        let ok = if index == last {
            if want_dir { meta.is_dir() } else { meta.is_file() }
        } else {
            meta.is_dir()
        };
        if !ok {
            return None;
        }
    }
    Some(path)
}

/// The host name to tag `--user` records with: the `--host` override, else the
/// system hostname.
fn resolve_host(cli: &Cli) -> anyhow::Result<String> {
    if let Some(host) = &cli.host {
        return Ok(host.clone());
    }
    let raw = nix::unistd::gethostname().context("resolving the system host name")?;
    Ok(raw.to_string_lossy().into_owned())
}

/// Index one code checkout via search-core's content-addressed reconcile
/// (Mixedbread only — code lives in git, not the parquet archive). Reuses the
/// exact code sync a bare `search` would run, so records are byte-identical
/// (same hashes, same repo scoping).
async fn index_code(
    label: &str,
    repo_dir: &std::path::Path,
    mixedbread: Option<(&MixedbreadStore, &str)>,
) -> anyhow::Result<()> {
    let Some((store, store_name)) = mixedbread else {
        anyhow::bail!("--code-repo requires --mixedbread-store (code is semantic-search only)");
    };
    let manifest = search_core::Manifest::build(repo_dir, None, MAX_FILE_BYTES)
        .with_context(|| format!("building manifest for {}", repo_dir.display()))?;
    let repo = search_core::repo_slug(repo_dir);
    let report = search_core::sync(store, store_name, repo_dir, &manifest, &repo, MAX_FILES, |_, _| {})
        .await
        .with_context(|| format!("[{label}] code sync"))?;
    if report.uploaded > 0 {
        search_core::wait_until_indexed(store, store_name, INDEX_TIMEOUT, |_| {})
            .await
            .with_context(|| format!("[{label}] waiting for indexing"))?;
    }
    eprintln!(
        "[{label}] mixedbread: uploaded {}, skipped {} of {}",
        report.uploaded, report.skipped, report.total
    );
    Ok(())
}

/// Record one source's outcome. A failure is logged and counted but does not
/// abort the run, so one broken source cannot starve the others.
fn record(label: &str, result: anyhow::Result<()>, indexed: &mut usize, failures: &mut usize) {
    match result {
        Ok(()) => *indexed += 1,
        Err(error) => {
            eprintln!("[{label}] failed: {error:#}");
            *failures += 1;
        }
    }
}

/// Fan one source out to every enabled sink.
async fn run_source<A: SourceAdapter + Sync>(
    label: &str,
    adapter: &A,
    mixedbread: Option<(&MixedbreadStore, &str)>,
    parquet: Option<&sink_parquet::Config>,
) -> anyhow::Result<()> {
    if let Some((store, store_name)) = mixedbread {
        let report = sync_documents(adapter, store, store_name, INDEX_TIMEOUT, |_, _| {})
            .await
            .with_context(|| format!("[{label}] Mixedbread sync"))?;
        eprintln!(
            "[{label}] mixedbread: uploaded {}, skipped {} of {}",
            report.uploaded, report.skipped, report.total
        );
    }
    if let Some(config) = parquet {
        let report =
            sink_parquet::sync(adapter, config).await.with_context(|| format!("[{label}] parquet sync"))?;
        if report.skipped {
            eprintln!("[{label}] parquet: skipped (unchanged)");
        } else {
            eprintln!("[{label}] parquet: wrote {} rows", report.rows);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![expect(clippy::expect_used, reason = "tests assert observable filesystem outcomes")]

    use std::path::Path;

    use super::{parse_user, safe_path_under};

    #[test]
    fn safe_path_accepts_real_nested_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join(".claude").join("projects")).expect("mkdir");
        assert!(safe_path_under(temp.path(), &[".claude", "projects"], true).is_some());
    }

    #[test]
    fn safe_path_rejects_symlinked_leaf() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path();
        std::fs::create_dir_all(home.join(".codex")).expect("mkdir");
        let secret = home.join("secret");
        std::fs::write(&secret, b"x").expect("write");
        std::os::unix::fs::symlink(&secret, home.join(".codex").join("history.jsonl")).expect("symlink");
        assert!(safe_path_under(home, &[".codex", "history.jsonl"], false).is_none());
    }

    #[test]
    fn safe_path_rejects_symlinked_ancestor() {
        // The privileged threat: a user points an ancestor component at another
        // tree so the root process reads through it.
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path();
        let victim = home.join("victim");
        std::fs::create_dir_all(victim.join("projects")).expect("mkdir");
        std::fs::write(victim.join("projects").join("s.jsonl"), b"{}").expect("write");
        std::os::unix::fs::symlink(&victim, home.join(".claude")).expect("symlink");
        assert!(
            safe_path_under(home, &[".claude", "projects"], true).is_none(),
            "a symlinked ancestor component must be rejected"
        );
    }

    #[test]
    fn safe_path_missing_is_none() {
        let temp = tempfile::tempdir().expect("tempdir");
        assert!(safe_path_under(temp.path(), &[".codex", "history.jsonl"], false).is_none());
    }

    #[test]
    fn safe_path_rejects_wrong_kind() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join(".claude").join("projects")).expect("mkdir");
        // `projects` is a directory, but a file was requested.
        assert!(safe_path_under(temp.path(), &[".claude", "projects"], false).is_none());
    }

    #[test]
    fn parse_user_validates_name_and_spec() {
        assert!(parse_user("a/b:/home/x").is_err(), "slash in name");
        assert!(parse_user("a=b:/home/x").is_err(), "equals in name");
        assert!(parse_user(":/home/x").is_err(), "empty name");
        assert!(parse_user("alice:").is_err(), "empty home");
        assert!(parse_user("noseparator").is_err(), "missing colon");
        let (name, home) = parse_user("alice-1.2_3:/home/alice").expect("valid spec");
        assert_eq!(name, "alice-1.2_3");
        assert_eq!(home, Path::new("/home/alice"));
    }
}
