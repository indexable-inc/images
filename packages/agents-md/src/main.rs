use std::{
    env, fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, ValueEnum};
use serde::Deserialize;
use similar::TextDiff;

const DOCUMENTS_ENV: &str = "AGENTS_MD_DOCUMENTS";
const DELTA_ENV: &str = "AGENTS_MD_DELTA";

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Render and validate generated Codex and Claude instruction files"
)]
struct Cli {
    /// Limit the command to one instruction target.
    #[arg(long, value_enum, default_value_t = TargetArg::All)]
    target: TargetArg,

    /// Write generated files. With no path, writes into the current directory.
    #[arg(long, value_name = "PATH", num_args = 0..=1, default_missing_value = ".")]
    write: Option<PathBuf>,

    /// Check generated files. With no path, checks the current directory.
    #[arg(long, value_name = "PATH", num_args = 0..=1, default_missing_value = ".")]
    check: Option<PathBuf>,

    /// Print one generated file to stdout. Requires a single selected target.
    #[arg(long)]
    print: bool,

    /// Render default diffs as plain unified diff text or through delta.
    #[arg(long, value_enum, default_value_t = DiffRenderer::Auto)]
    diff_renderer: DiffRenderer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum TargetArg {
    All,
    Codex,
    Claude,
}

impl TargetArg {
    const fn as_str(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            Self::Codex => Some("codex"),
            Self::Claude => Some("claude"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum DiffRenderer {
    Auto,
    Plain,
    Delta,
}

#[derive(Debug, Deserialize)]
struct Document {
    target: String,
    file_name: String,
    generated_path: PathBuf,
}

enum Mode {
    Diff,
    Check(PathBuf),
    Write(PathBuf),
    Print,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mode = mode_for(&cli)?;
    let documents = load_documents()?;

    match mode {
        Mode::Diff => diff_documents(&documents, cli.target, Path::new("."), cli.diff_renderer),
        Mode::Check(path) => check_documents(&documents, cli.target, &path),
        Mode::Write(path) => write_documents(&documents, cli.target, &path),
        Mode::Print => print_document(&documents, cli.target),
    }
}

fn mode_for(cli: &Cli) -> Result<Mode> {
    let selected_modes =
        u8::from(cli.write.is_some()) + u8::from(cli.check.is_some()) + u8::from(cli.print);
    ensure!(
        selected_modes <= 1,
        "--write, --check, and --print are mutually exclusive"
    );

    match (&cli.write, &cli.check, cli.print) {
        (Some(path), None, false) => Ok(Mode::Write(path.clone())),
        (None, Some(path), false) => Ok(Mode::Check(path.clone())),
        (None, None, true) => Ok(Mode::Print),
        (None, None, false) => Ok(Mode::Diff),
        _ => unreachable!("mode exclusivity checked before matching"),
    }
}

fn load_documents() -> Result<Vec<Document>> {
    let config_path = env::var_os(DOCUMENTS_ENV)
        .map(PathBuf::from)
        .with_context(|| format!("{DOCUMENTS_ENV} is not set"))?;
    let config = fs::read_to_string(&config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let documents: Vec<Document> = serde_json::from_str(&config)
        .with_context(|| format!("parsing {}", config_path.display()))?;

    ensure!(
        !documents.is_empty(),
        "{DOCUMENTS_ENV} does not list any documents"
    );
    for document in &documents {
        ensure!(!document.target.is_empty(), "document target is empty");
        ensure!(
            !document.file_name.is_empty(),
            "document file name is empty for target {}",
            document.target
        );
    }

    Ok(documents)
}

fn diff_documents(
    documents: &[Document],
    target: TargetArg,
    root: &Path,
    renderer: DiffRenderer,
) -> Result<()> {
    let selected = select_documents(documents, target, Some(root))?;
    let selected_count = selected.len();
    let mut changed = false;
    let mut patch = String::new();

    for document in selected {
        let current_path = destination_path(document, root, selected_count);
        let generated = generated_text(document)?;
        let current = read_current_text(&current_path)?;
        if current != generated {
            changed = true;
            patch.push_str(&unified_diff(document, &current_path, &current, &generated));
        }
    }

    if changed {
        write_diff(&patch, renderer)?;
    } else {
        println!("generated instruction files are up to date");
    }

    Ok(())
}

fn check_documents(documents: &[Document], target: TargetArg, path: &Path) -> Result<()> {
    let selected = select_documents(documents, target, Some(path))?;
    let selected_count = selected.len();
    let mut stale_paths = Vec::new();

    for document in selected {
        let current_path = destination_path(document, path, selected_count);
        let metadata = fs::symlink_metadata(&current_path)
            .with_context(|| format!("reading metadata for {}", current_path.display()))?;
        if metadata.file_type().is_symlink() {
            stale_paths.push(current_path);
            continue;
        }

        let generated = generated_text(document)?;
        let current = fs::read_to_string(&current_path)
            .with_context(|| format!("reading {}", current_path.display()))?;
        if current != generated {
            stale_paths.push(current_path);
        }
    }

    if stale_paths.is_empty() {
        return Ok(());
    }

    for path in &stale_paths {
        eprintln!("{} differs from generated content", path.display());
    }
    bail!("generated instruction files are stale");
}

fn write_documents(documents: &[Document], target: TargetArg, path: &Path) -> Result<()> {
    let selected = select_documents(documents, target, Some(path))?;
    let selected_count = selected.len();

    for document in selected {
        let output_path = destination_path(document, path, selected_count);
        write_document(&output_path, &generated_text(document)?)?;
        println!("wrote {}", output_path.display());
    }

    Ok(())
}

fn print_document(documents: &[Document], target: TargetArg) -> Result<()> {
    let selected = select_documents(documents, target, None)?;
    ensure!(
        selected.len() == 1,
        "--print requires --target codex or --target claude"
    );

    print!("{}", generated_text(selected[0])?);
    Ok(())
}

fn select_documents<'a>(
    documents: &'a [Document],
    target: TargetArg,
    path_hint: Option<&Path>,
) -> Result<Vec<&'a Document>> {
    if let Some(target_name) = target.as_str() {
        let selected: Vec<_> = documents
            .iter()
            .filter(|document| document.target == target_name)
            .collect();
        ensure!(
            !selected.is_empty(),
            "no generated document is configured for target {target_name}"
        );
        return Ok(selected);
    }

    if let Some(path) = path_hint {
        if let Some(document) = infer_document_from_path(documents, path) {
            return Ok(vec![document]);
        }

        if matches!(existing_path_is_file(path), Some(true)) {
            bail!(
                "cannot infer instruction target from {}; pass --target codex or --target claude",
                path.display()
            );
        }
    }

    Ok(documents.iter().collect())
}

fn infer_document_from_path<'a>(documents: &'a [Document], path: &Path) -> Option<&'a Document> {
    let file_name = path.file_name()?.to_str()?;
    documents
        .iter()
        .find(|document| document.file_name == file_name)
}

fn destination_path(document: &Document, path: &Path, selected_count: usize) -> PathBuf {
    if selected_count == 1 && path_looks_like_file(path, document) {
        path.to_path_buf()
    } else {
        path.join(&document.file_name)
    }
}

fn path_looks_like_file(path: &Path, document: &Document) -> bool {
    if let Some(is_file) = existing_path_is_file(path) {
        return is_file;
    }

    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == document.file_name)
}

fn existing_path_is_file(path: &Path) -> Option<bool> {
    fs::metadata(path).map(|metadata| !metadata.is_dir()).ok()
}

fn generated_text(document: &Document) -> Result<String> {
    fs::read_to_string(&document.generated_path)
        .with_context(|| format!("reading {}", document.generated_path.display()))
}

fn read_current_text(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn unified_diff(document: &Document, path: &Path, current: &str, generated: &str) -> String {
    let old_header = format!("current/{}", path.display());
    let new_header = format!("generated/{}", document.file_name);
    TextDiff::from_lines(current, generated)
        .unified_diff()
        .header(&old_header, &new_header)
        .to_string()
}

fn write_diff(patch: &str, renderer: DiffRenderer) -> Result<()> {
    match renderer {
        DiffRenderer::Delta => write_delta(patch),
        DiffRenderer::Auto if io::stdout().is_terminal() => {
            if write_delta(patch).is_err() {
                print!("{patch}");
            }
            Ok(())
        }
        DiffRenderer::Plain | DiffRenderer::Auto => {
            print!("{patch}");
            Ok(())
        }
    }
}

fn write_delta(patch: &str) -> Result<()> {
    let delta = env::var_os(DELTA_ENV).map_or_else(|| PathBuf::from("delta"), PathBuf::from);
    let mut child = Command::new(&delta)
        .arg("--paging=never")
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting {}", delta.display()))?;
    let mut stdin = child.stdin.take().context("opening delta stdin")?;
    stdin
        .write_all(patch.as_bytes())
        .context("writing diff to delta")?;
    drop(stdin);

    let status = child.wait().context("waiting for delta")?;
    ensure!(status.success(), "delta exited with {status}");
    Ok(())
}

fn write_document(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }

    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        fs::remove_file(path).with_context(|| format!("removing symlink {}", path.display()))?;
    }

    fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn documents() -> Vec<Document> {
        vec![
            Document {
                target: "codex".to_owned(),
                file_name: "AGENTS.md".to_owned(),
                generated_path: PathBuf::from("/generated/AGENTS.md"),
            },
            Document {
                target: "claude".to_owned(),
                file_name: "CLAUDE.md".to_owned(),
                generated_path: PathBuf::from("/generated/CLAUDE.md"),
            },
        ]
    }

    #[test]
    fn all_target_infers_codex_from_agents_path() {
        let documents = documents();
        let selected = select_documents(&documents, TargetArg::All, Some(Path::new("AGENTS.md")))
            .expect("selection succeeds");

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].target, "codex");
    }

    #[test]
    fn all_target_keeps_directory_paths_as_all_documents() {
        let documents = documents();
        let selected = select_documents(&documents, TargetArg::All, Some(Path::new(".")))
            .expect("selection succeeds");

        assert_eq!(selected.len(), 2);
    }

    #[test]
    fn single_target_writes_file_paths_directly() {
        let documents = documents();
        let path = destination_path(&documents[0], Path::new("AGENTS.md"), 1);

        assert_eq!(path, PathBuf::from("AGENTS.md"));
    }

    #[test]
    fn multi_target_treats_path_as_directory() {
        let documents = documents();
        let path = destination_path(&documents[1], Path::new("."), 2);

        assert_eq!(path, PathBuf::from("./CLAUDE.md"));
    }

    #[test]
    fn existing_dotted_directory_is_not_a_file_path() {
        let temp_dir = tempfile::tempdir().expect("temp dir is created");
        let dotted_dir = temp_dir.path().join("checkout.with.dots");
        fs::create_dir(&dotted_dir).expect("dotted dir is created");
        let documents = documents();

        assert!(!path_looks_like_file(&dotted_dir, &documents[0]));
    }

    #[test]
    fn all_target_treats_missing_dotted_path_as_directory() {
        let documents = documents();
        let selected = select_documents(
            &documents,
            TargetArg::All,
            Some(Path::new("checkout.with.dots")),
        )
        .expect("selection succeeds");

        assert_eq!(selected.len(), 2);
    }

    #[test]
    fn single_target_treats_missing_dotted_path_as_directory() {
        let documents = documents();
        let path = destination_path(&documents[0], Path::new("checkout.with.dots"), 1);

        assert_eq!(path, PathBuf::from("checkout.with.dots/AGENTS.md"));
    }
}
