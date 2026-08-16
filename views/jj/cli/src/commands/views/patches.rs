// Copyright 2026 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::fs;
use std::fs::File;
use std::io::Read as _;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use clap_complete::ArgValueCandidates;
use gix::ObjectId;
use jj_lib::object_id::ObjectId as _;
use jj_views::Cache;
use sha2::Digest as _;
use sha2::Sha256;
use tracing::instrument;

use super::ViewConfig;
use super::commit_tree;
use super::get_views_config;
use super::lift_error;
use super::open_store;
use super::select_views;
use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::CommandError;
use crate::command_error::user_error;
use crate::command_error::user_error_with_message;
use crate::complete;
use crate::ui::Ui;

const ARCHIVE_COMPRESSION_LEVEL: i32 = 3;
const TAR_BLOCK_BYTES: usize = 512;

/// Export one anchored view as an ordered patch archive
#[derive(clap::Args, Clone, Debug)]
pub struct ViewsPatchesArgs {
    /// View to export, by its key in `.jj-views.toml`
    #[arg(value_name = "VIEW")]
    #[arg(add = ArgValueCandidates::new(complete::views))]
    view: String,

    /// Host revision whose view should be exported
    #[arg(long, short = 'r', value_name = "HOST_REV")]
    revision: RevisionArg,

    /// Existing empty directory that will receive patches and `manifest.json`
    #[arg(long, value_name = "EMPTY_DIR")]
    output: PathBuf,

    /// Deterministic tar.zst archive to write or verify
    #[arg(long, value_name = "PATH.tar.zst")]
    archive: PathBuf,

    /// Emit the canonical manifest JSON to stdout
    #[arg(long)]
    json: bool,
}

#[derive(Debug, serde::Serialize)]
struct PatchManifest {
    view: String,
    host_revision: String,
    anchor_source: String,
    anchor_view: String,
    anchor_tree: String,
    view_tree: String,
    commit_ids: Vec<String>,
    patches: Vec<PatchRecord>,
    patch_count: usize,
    archive_path: String,
    archive_sha256: String,
}

#[derive(Debug, serde::Serialize)]
struct PatchRecord {
    path: String,
    sha256: String,
}

#[instrument(skip_all)]
pub async fn cmd_views_patches(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ViewsPatchesArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui).await?;
    let configured = get_views_config(&workspace_command).await?;
    let selected = select_views(&configured, std::slice::from_ref(&args.view))?;
    let view = selected[0];
    let anchor = view.anchor.ok_or_else(|| {
        user_error(format!(
            "The {} view has no anchor, so its patch range is undefined",
            view.name
        ))
        .hinted(format!(
            "Add `[views.{}.anchor]` with full `source` and `view` commit ids.",
            view.name
        ))
    })?;
    let host_commit = workspace_command
        .resolve_single_rev(ui, &args.revision)
        .await?;
    let (git, repo) = open_store(&workspace_command, "patches")?;
    // Only after the store check: on a backend that keeps no Git objects this
    // commit id is not one, and reading it as though it were is how a backend's
    // choice of hash width used to abort the process.
    let host_revision = ObjectId::from_bytes_or_panic(host_commit.id().as_bytes());
    let filter = view.filter()?;
    let mut cache = Cache::new();
    cache
        .seed_anchor(&repo, &host_revision, &filter, anchor)
        .map_err(|err| lift_error(view, err))?;
    let view_tip = jj_views::derive(&repo, &host_revision, &filter, &mut cache)
        .map_err(|err| lift_error(view, err))?
        .ok_or_else(|| {
            user_error(format!(
                "The {} view has no history at {host_revision}",
                view.name
            ))
        })?;
    let commits = linear_commits(&repo, view, anchor.view, view_tip)?;

    let paths = CheckedOutputPaths::new(&args.output, &args.archive)?;
    let staging = tempfile::Builder::new()
        .prefix(".jj-view-patches-")
        .tempdir_in(&paths.output_parent)
        .map_err(|err| io_error("Could not create the patch staging directory", err))?;
    let patch_dir = staging.path().join("patches");
    fs::create_dir(&patch_dir)
        .map_err(|err| io_error("Could not create the staged patch directory", err))?;

    let patches = write_patches(&git, &commits, &patch_dir)?;
    let archive_file = tempfile::NamedTempFile::new_in(&paths.archive_parent)
        .map_err(|err| io_error("Could not create the staged archive", err))?;
    let archive_file = write_archive(archive_file, staging.path(), &patches)?;
    let archive_hash = sha256_file(archive_file.path())?;
    check_existing_archive(&args.archive, archive_file.path())?;

    let manifest = PatchManifest {
        view: view.name.clone(),
        host_revision: host_revision.to_string(),
        anchor_source: anchor.source.to_string(),
        anchor_view: anchor.view.to_string(),
        anchor_tree: commit_tree(&repo, anchor.view)?.to_string(),
        view_tree: commit_tree(&repo, view_tip)?.to_string(),
        commit_ids: commits.iter().map(ToString::to_string).collect(),
        patch_count: patches.len(),
        patches,
        archive_path: args.archive.to_string_lossy().into_owned(),
        archive_sha256: archive_hash,
    };
    let mut manifest_json = serde_json::to_vec(&manifest)
        .map_err(|err| user_error_with_message("Could not encode the patch manifest", err))?;
    manifest_json.push(b'\n');
    fs::write(staging.path().join("manifest.json"), &manifest_json)
        .map_err(|err| io_error("Could not write the patch manifest", err))?;

    if !args.archive.exists() {
        archive_file
            .persist(&args.archive)
            .map_err(|err| io_error("Could not install the patch archive", err.error))?;
    }
    let staging_path = staging.keep();
    fs::remove_dir(&args.output)
        .map_err(|err| io_error("Could not replace the empty output directory", err))?;
    fs::rename(&staging_path, &args.output).map_err(|err| {
        io_error(
            format!(
                "Could not install the patch output from {}",
                staging_path.display()
            ),
            err,
        )
    })?;

    if args.json {
        ui.stdout().write_all(&manifest_json)?;
    } else {
        writeln!(
            ui.stdout(),
            "Exported {} patches for {} at {} to {} ({})",
            manifest.patch_count,
            manifest.view,
            manifest.host_revision,
            manifest.archive_path,
            manifest.archive_sha256
        )?;
    }
    Ok(())
}

struct CheckedOutputPaths {
    output_parent: PathBuf,
    archive_parent: PathBuf,
}

impl CheckedOutputPaths {
    fn new(output: &Path, archive: &Path) -> Result<Self, CommandError> {
        let metadata = fs::symlink_metadata(output)
            .map_err(|err| io_error(format!("Could not inspect {}", output.display()), err))?;
        if !metadata.file_type().is_dir() {
            return Err(user_error(format!(
                "Patch output {} is not a directory",
                output.display()
            )));
        }
        let mut entries = fs::read_dir(output)
            .map_err(|err| io_error(format!("Could not read {}", output.display()), err))?;
        if entries
            .next()
            .transpose()
            .map_err(|err| io_error(format!("Could not read {}", output.display()), err))?
            .is_some()
        {
            return Err(user_error(format!(
                "Patch output {} is not empty",
                output.display()
            )));
        }
        let output = output
            .canonicalize()
            .map_err(|err| io_error("Could not resolve the patch output directory", err))?;
        let output_parent = output
            .parent()
            .ok_or_else(|| user_error("The patch output directory has no parent"))?
            .to_owned();
        let archive_parent = archive
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .canonicalize()
            .map_err(|err| io_error("Could not resolve the archive parent directory", err))?;
        let archive_name = archive
            .file_name()
            .ok_or_else(|| user_error("The archive path has no file name"))?;
        let resolved_archive = archive_parent.join(archive_name);
        if resolved_archive.starts_with(&output) {
            return Err(user_error(format!(
                "Archive {} is inside the patch output it would archive",
                archive.display()
            )));
        }
        Ok(Self {
            output_parent,
            archive_parent,
        })
    }
}

fn linear_commits(
    repo: &gix::Repository,
    view: &ViewConfig,
    anchor: ObjectId,
    tip: ObjectId,
) -> Result<Vec<ObjectId>, CommandError> {
    let commits = jj_views::verify::ancestry_after(repo, &tip, &anchor)
        .map_err(|err| lift_error(view, err))?;
    let mut expected_parent = anchor;
    for commit in &commits {
        let raw = repo
            .find_object(*commit)
            .map_err(|err| {
                user_error_with_message(format!("Could not read view commit {commit}"), err)
            })?
            .detach()
            .data;
        let parsed = gix::objs::CommitRef::from_bytes(&raw, repo.object_hash()).map_err(|err| {
            user_error_with_message(format!("Could not parse view commit {commit}"), err)
        })?;
        let parents: Vec<ObjectId> = parsed.parents().collect();
        if parents.as_slice() != [expected_parent] {
            return Err(user_error(format!(
                "View commit {commit} does not form a linear patch series after {expected_parent}"
            ))
            .hinted(
                "Patch archives preserve commit diffs, not merge topology. Rebase the unpublished \
                 view commits into one line before exporting.",
            ));
        }
        expected_parent = *commit;
    }
    Ok(commits)
}

fn write_patches(
    git: &super::Git,
    commits: &[ObjectId],
    output: &Path,
) -> Result<Vec<PatchRecord>, CommandError> {
    commits
        .iter()
        .enumerate()
        .map(|(index, commit)| {
            let patch = git.format_patch(*commit).map_err(|err| {
                user_error(format!("Could not format view commit {commit}: {err}"))
            })?;
            if patch.is_empty() {
                return Err(user_error(format!(
                    "Git emitted no patch for view commit {commit}"
                )));
            }
            let path = format!("patches/{:04}-{commit}.patch", index + 1);
            fs::write(output.join(path.trim_start_matches("patches/")), &patch)
                .map_err(|err| io_error(format!("Could not write {path}"), err))?;
            Ok(PatchRecord {
                path,
                sha256: sha256_bytes(&patch),
            })
        })
        .collect()
}

fn write_archive(
    file: tempfile::NamedTempFile,
    root: &Path,
    patches: &[PatchRecord],
) -> Result<tempfile::NamedTempFile, CommandError> {
    let encoder = zstd::stream::write::Encoder::new(file, ARCHIVE_COMPRESSION_LEVEL)
        .map_err(|err| io_error("Could not start the archive compressor", err))?;
    let mut archive = DeterministicTar::new(encoder);
    for patch in patches {
        archive.append(root, &patch.path)?;
    }
    let encoder = archive.finish()?;
    encoder
        .finish()
        .map_err(|err| io_error("Could not finish the patch archive", err))
}

struct DeterministicTar<W> {
    writer: W,
}

impl<W: Write> DeterministicTar<W> {
    fn new(writer: W) -> Self {
        Self { writer }
    }

    fn append(&mut self, root: &Path, path: &str) -> Result<(), CommandError> {
        let metadata = fs::metadata(root.join(path))
            .map_err(|err| io_error(format!("Could not inspect {path}"), err))?;
        let size = metadata.len();
        let header = tar_header(path, size)?;
        self.writer
            .write_all(&header)
            .map_err(|err| io_error(format!("Could not archive {path}"), err))?;
        let mut file = File::open(root.join(path))
            .map_err(|err| io_error(format!("Could not read {path}"), err))?;
        std::io::copy(&mut file, &mut self.writer)
            .map_err(|err| io_error(format!("Could not archive {path}"), err))?;
        let padding =
            (TAR_BLOCK_BYTES as u64 - size % TAR_BLOCK_BYTES as u64) % TAR_BLOCK_BYTES as u64;
        self.writer
            .write_all(&[0; TAR_BLOCK_BYTES][..padding as usize])
            .map_err(|err| io_error(format!("Could not pad {path} in the archive"), err))?;
        Ok(())
    }

    fn finish(mut self) -> Result<W, CommandError> {
        self.writer
            .write_all(&[0; TAR_BLOCK_BYTES * 2])
            .map_err(|err| io_error("Could not finish the tar stream", err))?;
        Ok(self.writer)
    }
}

fn tar_header(path: &str, size: u64) -> Result<[u8; TAR_BLOCK_BYTES], CommandError> {
    let path = path.as_bytes();
    if path.len() > 100 {
        return Err(user_error(format!(
            "Archive path is longer than the ustar name field: {} bytes",
            path.len()
        )));
    }
    let mut header = [0_u8; TAR_BLOCK_BYTES];
    header[..path.len()].copy_from_slice(path);
    write_octal(&mut header[100..108], 0o644)?;
    write_octal(&mut header[108..116], 0)?;
    write_octal(&mut header[116..124], 0)?;
    write_octal(&mut header[124..136], size)?;
    write_octal(&mut header[136..148], 0)?;
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
    let encoded = format!("{checksum:06o}\0 ");
    header[148..156].copy_from_slice(encoded.as_bytes());
    Ok(header)
}

fn write_octal(field: &mut [u8], value: u64) -> Result<(), CommandError> {
    let encoded = format!("{:0width$o}\0", value, width = field.len() - 1);
    if encoded.len() != field.len() {
        return Err(user_error(format!(
            "Value {value} does not fit in a {} byte tar field",
            field.len()
        )));
    }
    field.copy_from_slice(encoded.as_bytes());
    Ok(())
}

fn check_existing_archive(existing: &Path, candidate: &Path) -> Result<(), CommandError> {
    if !existing.exists() {
        return Ok(());
    }
    if files_equal(existing, candidate)? {
        return Ok(());
    }
    Err(user_error(format!(
        "Existing archive {} differs from the derived artifact",
        existing.display()
    )))
}

fn files_equal(left: &Path, right: &Path) -> Result<bool, CommandError> {
    let left_size = fs::metadata(left)
        .map_err(|err| io_error(format!("Could not inspect {}", left.display()), err))?
        .len();
    let right_size = fs::metadata(right)
        .map_err(|err| io_error(format!("Could not inspect {}", right.display()), err))?
        .len();
    if left_size != right_size {
        return Ok(false);
    }
    let mut left = File::open(left)
        .map_err(|err| io_error(format!("Could not read {}", left.display()), err))?;
    let mut right = File::open(right)
        .map_err(|err| io_error(format!("Could not read {}", right.display()), err))?;
    let mut left_chunk = [0_u8; 64 * 1024];
    let mut right_chunk = [0_u8; 64 * 1024];
    loop {
        let left_read = left
            .read(&mut left_chunk)
            .map_err(|err| io_error("Could not compare the existing archive", err))?;
        let right_read = right
            .read(&mut right_chunk)
            .map_err(|err| io_error("Could not compare the derived archive", err))?;
        if left_read != right_read || left_chunk[..left_read] != right_chunk[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
    }
}

fn sha256_file(path: &Path) -> Result<String, CommandError> {
    let mut file = File::open(path)
        .map_err(|err| io_error(format!("Could not read {}", path.display()), err))?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash)
        .map_err(|err| io_error(format!("Could not hash {}", path.display()), err))?;
    Ok(format!("{:x}", hash.finalize()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn io_error(
    message: impl Into<String>,
    error: impl Into<Box<dyn std::error::Error + Send + Sync>>,
) -> CommandError {
    user_error_with_message(message.into(), error)
}

impl super::Git {
    fn format_patch(&self, commit: ObjectId) -> Result<Vec<u8>, String> {
        let mut command = self.command();
        command.args([
            "-c",
            "format.useAutoBase=false",
            "-c",
            "i18n.logOutputEncoding=UTF-8",
            "format-patch",
            "--stdout",
            "--no-signature",
            "--no-stat",
            "--no-base",
            "--no-numbered",
            "--full-index",
            "--binary",
            "--find-renames=50%",
            "--no-ext-diff",
            "--no-textconv",
            "-1",
        ]);
        command.arg(commit.to_string());
        let output = command
            .output()
            .map_err(|err| format!("could not run {}: {err}", self.executable.display()))?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            let message = String::from_utf8_lossy(&output.stderr)
                .trim_end()
                .to_owned();
            Err(if message.is_empty() {
                format!("git exited with {}", output.status)
            } else {
                message
            })
        }
    }
}
